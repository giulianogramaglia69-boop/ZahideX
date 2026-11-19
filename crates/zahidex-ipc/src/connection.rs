// SPDX-License-Identifier: GPL-3.0-or-later
use std::io::{self, IoSlice, IoSliceMut, Write};
use std::os::unix::io::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use nix::sys::socket::{ControlMessage, ControlMessageOwned, MsgFlags, UnixAddr, recvmsg, sendmsg};
use serde::{Serialize, de::DeserializeOwned};
use zahidex_protocol::{ClientMessage, MAGIC, MAX_FRAME_BYTES, ProtocolError, Rect, WindowId};

use crate::error::IpcError;
use crate::shm::ShmBuffer;

/// Numero massimo di file descriptor accettati in un singolo messaggio.
const MAX_FDS_PER_MESSAGE: usize = 4;

/// Lunghezza dell'header di un frame (magic + lunghezza).
const HEADER_LEN: usize = 8;

/// Una connessione ZahideX: uno Unix stream + framing del protocollo.
///
/// Gli fd ricevuti via SCM_RIGHTS vengono restituiti insieme al messaggio
/// a cui appartengono (vedere [`Connection::recv`]).
#[derive(Debug)]
pub struct Connection {
    stream: UnixStream,
    pending_fds: Vec<OwnedFd>,
}

impl Connection {
    /// Avvolge uno UnixStream già connesso.
    pub fn from_stream(stream: UnixStream) -> Self {
        Self {
            stream,
            pending_fds: Vec::new(),
        }
    }

    /// Connette al server ZahideX sul socket indicato.
    pub fn connect(path: &std::path::Path) -> Result<Self, IpcError> {
        let stream = UnixStream::connect(path)?;
        Ok(Self::from_stream(stream))
    }

    /// Imposta il timeout di lettura (None = bloccante senza timeout).
    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> Result<(), IpcError> {
        self.stream.set_read_timeout(timeout)?;
        Ok(())
    }

    /// Imposta il timeout di scrittura (None = bloccante senza timeout).
    pub fn set_write_timeout(&self, timeout: Option<Duration>) -> Result<(), IpcError> {
        self.stream.set_write_timeout(timeout)?;
        Ok(())
    }

    /// Invia un messaggio (senza fd).
    pub fn send<T: Serialize>(&mut self, msg: &T) -> Result<(), IpcError> {
        let frame = zahidex_protocol::encode_frame(msg)?;
        self.stream.write_all(&frame)?;
        Ok(())
    }

    /// Invia un messaggio allegando file descriptor via SCM_RIGHTS.
    pub fn send_with_fds<T: Serialize>(
        &mut self,
        msg: &T,
        fds: &[BorrowedFd],
    ) -> Result<(), IpcError> {
        let frame = zahidex_protocol::encode_frame(msg)?;
        send_frame_with_fds(&self.stream, &frame, fds)?;
        Ok(())
    }

    /// Invia un `CommitBuffer` per la finestra indicata, allegando l'fd del buffer.
    pub fn send_commit_buffer(
        &mut self,
        window_id: WindowId,
        buffer: &ShmBuffer,
        damage: Rect,
    ) -> Result<(), IpcError> {
        let msg = ClientMessage::CommitBuffer {
            window_id,
            buffer: buffer.buffer_info(),
            damage,
        };
        self.send_with_fds(&msg, &[buffer.as_fd()])
    }

    /// Attende finché lo stream ha dati da leggere (o scade il timeout).
    ///
    /// Restituisce `true` se ci sono dati da leggere con `recv`,
    /// `false` in caso di timeout. Un errore di poll o la chiusura dello
    /// stream (`POLLHUP`/`POLLERR`) diventano `IpcError` — ma solo se non
    /// ci sono anche dati da leggere: se il peer ha inviato dati e poi ha
    /// chiuso (`POLLIN` + `POLLHUP` insieme), i dati restano leggibili e
    /// questa funzione restituisce `true`.
    ///
    /// Serve ai thread client per svuotare la coda dei messaggi in uscita
    /// mentre aspettano il prossimo messaggio, senza usare un timeout di
    /// lettura che potrebbe scattare a metà di un frame.
    pub fn wait_readable(&self, timeout: Duration) -> Result<bool, IpcError> {
        use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
        use std::os::unix::io::AsFd;
        let mut fds = [PollFd::new(self.stream.as_fd(), PollFlags::POLLIN)];
        let millis = timeout.as_millis().min(i32::MAX as u128) as u32;
        let timeout = PollTimeout::try_from(millis)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        let ready = poll(&mut fds, timeout)?;
        if ready == 0 {
            return Ok(false);
        }
        let revents = fds[0].revents().unwrap_or(PollFlags::empty());
        // Se ci sono dati da leggere, restituiscili anche se il peer ha già
        // chiuso (POLLIN + POLLHUP insieme: i dati inviati prima della
        // chiusura restano leggibili).
        if revents.contains(PollFlags::POLLIN) {
            return Ok(true);
        }
        if revents.contains(PollFlags::POLLHUP)
            || revents.contains(PollFlags::POLLERR)
            || revents.contains(PollFlags::POLLNVAL)
        {
            return Err(IpcError::UnexpectedEof);
        }
        Ok(false)
    }

    /// Riceve il prossimo messaggio con gli eventuali fd allegati.
    ///
    /// Gli fd restituiti appartengono a questo messaggio: la lettura avviene
    /// con lunghezze esatte (prima l'header, poi il payload), così gli fd
    /// arrivati durante la lettura di un frame non possono "scivolare"
    /// sul messaggio successivo.
    pub fn recv<T: DeserializeOwned>(&mut self) -> Result<(T, Vec<OwnedFd>), IpcError> {
        let mut header = [0u8; HEADER_LEN];
        self.read_exact(&mut header)?;
        if header[..4] != MAGIC {
            let mut magic = [0u8; 4];
            magic.copy_from_slice(&header[..4]);
            return Err(ProtocolError::InvalidMagic(magic).into());
        }
        let len = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as usize;
        if len > MAX_FRAME_BYTES {
            return Err(ProtocolError::FrameTooLarge { len }.into());
        }
        let mut payload = vec![0u8; len];
        self.read_exact(&mut payload)?;
        let msg = postcard::from_bytes(&payload).map_err(ProtocolError::from)?;
        let fds = std::mem::take(&mut self.pending_fds);
        Ok((msg, fds))
    }

    /// Legge esattamente `buf.len()` byte dallo stream, accumulando in
    /// `pending_fds` gli fd ricevuti via SCM_RIGHTS durante la lettura.
    fn read_exact(&mut self, buf: &mut [u8]) -> Result<(), IpcError> {
        let mut offset = 0;
        while offset < buf.len() {
            let mut cmsg_buf = nix::cmsg_space!([RawFd; MAX_FDS_PER_MESSAGE]);
            let mut iov = [IoSliceMut::new(&mut buf[offset..])];
            let r = recvmsg::<UnixAddr>(
                self.stream.as_raw_fd(),
                &mut iov,
                Some(&mut cmsg_buf),
                MsgFlags::empty(),
            )
            .map_err(io::Error::from)?;
            if r.bytes == 0 {
                return Err(IpcError::UnexpectedEof);
            }
            for cmsg in r.cmsgs().map_err(|_| IpcError::ControlTruncated)? {
                if let ControlMessageOwned::ScmRights(fds) = cmsg {
                    // Gli fd arrivano dal kernel via SCM_RIGHTS: sono nostri,
                    // avvolgerli in OwnedFd ne garantisce la chiusura.
                    self.pending_fds.extend(
                        fds.into_iter()
                            .map(|fd| unsafe { OwnedFd::from_raw_fd(fd) }),
                    );
                }
            }
            offset += r.bytes;
        }
        Ok(())
    }
}

/// Invia `frame` con `sendmsg`, allegando gli fd solo alla prima chiamata
/// (le chiamate successive, se il frame viene scritto in più pezzi,
/// non portano dati ancillari).
fn send_frame_with_fds(
    stream: &UnixStream,
    frame: &[u8],
    fds: &[BorrowedFd],
) -> Result<(), IpcError> {
    let raw_fds: Vec<RawFd> = fds.iter().map(BorrowedFd::as_raw_fd).collect();
    let mut offset = 0;
    let mut first = true;
    while offset < frame.len() {
        let iov = [IoSlice::new(&frame[offset..])];
        let cmsgs: &[ControlMessage] = if first && !raw_fds.is_empty() {
            &[ControlMessage::ScmRights(&raw_fds)]
        } else {
            &[]
        };
        let sent = sendmsg::<UnixAddr>(stream.as_raw_fd(), &iov, cmsgs, MsgFlags::empty(), None)
            .map_err(io::Error::from)?;
        if sent == 0 {
            return Err(io::Error::new(io::ErrorKind::WriteZero, "sendmsg wrote 0 bytes").into());
        }
        offset += sent;
        first = false;
    }
    Ok(())
}
