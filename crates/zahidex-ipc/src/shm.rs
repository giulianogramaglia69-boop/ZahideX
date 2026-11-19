// SPDX-License-Identifier: GPL-3.0-or-later
use std::fs::File;
use std::mem::ManuallyDrop;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd};

use memmap2::{Mmap, MmapMut, MmapOptions};
use nix::sys::memfd::{MFdFlags, memfd_create};
use nix::sys::stat::fstat;
use nix::unistd::ftruncate;
use zahidex_protocol::{BufferInfo, PixelFormat};

use crate::error::IpcError;

/// Byte per pixel del formato v0.1 (XRGB8888).
pub const BYTES_PER_PIXEL: u32 = 4;

/// Un buffer condiviso (memfd) per i contenuti di una finestra.
///
/// Il client disegna nel buffer e passa l'fd al server via SCM_RIGHTS;
/// il server lo avvolge con [`ShmBuffer::from_fd`] e lo mappa in lettura.
#[derive(Debug)]
pub struct ShmBuffer {
    fd: OwnedFd,
    width: u32,
    height: u32,
    stride: u32,
    size: usize,
}

impl ShmBuffer {
    /// Crea un buffer `width` x `height` con stride minimo (`width * 4`).
    pub fn new(width: u32, height: u32) -> Result<Self, IpcError> {
        let stride = width
            .checked_mul(BYTES_PER_PIXEL)
            .ok_or(IpcError::InvalidDimensions {
                width,
                height,
                stride: 0,
            })?;
        Self::with_stride(width, height, stride)
    }

    /// Crea un buffer con stride esplicito (byte per riga, multiplo di 4).
    pub fn with_stride(width: u32, height: u32, stride: u32) -> Result<Self, IpcError> {
        validate_dimensions(width, height, stride)?;
        let size = stride as usize * height as usize;
        let fd = memfd_create(c"zahidex-shm", MFdFlags::MFD_CLOEXEC)?;
        ftruncate(fd.as_fd(), size as i64)?;
        Ok(Self {
            fd,
            width,
            height,
            stride,
            size,
        })
    }

    /// Avvolge un fd ricevuto via SCM_RIGHTS, verificando che la dimensione
    /// reale del memfd corrisponda a `stride * height`.
    pub fn from_fd(fd: OwnedFd, width: u32, height: u32, stride: u32) -> Result<Self, IpcError> {
        validate_dimensions(width, height, stride)?;
        let size = stride as usize * height as usize;
        let actual = fstat(fd.as_fd())?.st_size as usize;
        if actual != size {
            return Err(IpcError::InvalidBufferSize {
                expected: size,
                actual,
            });
        }
        Ok(Self {
            fd,
            width,
            height,
            stride,
            size,
        })
    }

    /// fd del buffer, da passare via SCM_RIGHTS.
    pub fn as_fd(&self) -> BorrowedFd<'_> {
        self.fd.as_fd()
    }

    /// Descrizione del buffer per il messaggio `CommitBuffer`.
    pub fn buffer_info(&self) -> BufferInfo {
        BufferInfo {
            width: self.width,
            height: self.height,
            stride: self.stride,
            format: PixelFormat::Xrgb8888,
        }
    }

    /// Mappa il buffer in lettura/scrittura.
    pub fn map_mut(&self) -> Result<MmapMut, IpcError> {
        let file = ManuallyDrop::new(unsafe { File::from_raw_fd(self.fd.as_raw_fd()) });
        let mmap = unsafe { MmapOptions::new().len(self.size).map_mut(&*file)? };
        Ok(mmap)
    }

    /// Mappa il buffer in sola lettura.
    pub fn map(&self) -> Result<Mmap, IpcError> {
        let file = ManuallyDrop::new(unsafe { File::from_raw_fd(self.fd.as_raw_fd()) });
        let mmap = unsafe { MmapOptions::new().len(self.size).map(&*file)? };
        Ok(mmap)
    }

    /// Larghezza in pixel.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Altezza in pixel.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Byte per riga.
    pub fn stride(&self) -> u32 {
        self.stride
    }

    /// Dimensione totale in byte (`stride * height`).
    pub fn size_bytes(&self) -> usize {
        self.size
    }
}

fn validate_dimensions(width: u32, height: u32, stride: u32) -> Result<(), IpcError> {
    let ok = width > 0
        && height > 0
        && stride >= width.saturating_mul(BYTES_PER_PIXEL)
        && stride.is_multiple_of(BYTES_PER_PIXEL)
        && (stride as usize).checked_mul(height as usize).is_some();
    if ok {
        Ok(())
    } else {
        Err(IpcError::InvalidDimensions {
            width,
            height,
            stride,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_map_and_read_back() {
        let buf = ShmBuffer::new(64, 64).unwrap();
        assert_eq!(buf.width(), 64);
        assert_eq!(buf.height(), 64);
        assert_eq!(buf.stride(), 256);
        assert_eq!(buf.size_bytes(), 64 * 256);
        {
            let mut map = buf.map_mut().unwrap();
            map.fill(0xAB);
            map[0] = 0x11;
            let len = map.len();
            map[len - 1] = 0x22;
        }
        let map = buf.map().unwrap();
        assert_eq!(map[0], 0x11);
        assert_eq!(map[1], 0xAB);
        assert_eq!(map[map.len() - 1], 0x22);
    }

    #[test]
    fn from_fd_wraps_a_duplicated_fd() {
        let buf = ShmBuffer::new(32, 16).unwrap();
        {
            let mut map = buf.map_mut().unwrap();
            map.fill(0x5A);
        }
        let dup = buf.as_fd().try_clone_to_owned().unwrap();
        let other = ShmBuffer::from_fd(dup, 32, 16, 128).unwrap();
        let map = other.map().unwrap();
        assert!(map.iter().all(|b| *b == 0x5A));
    }

    #[test]
    fn rejects_invalid_dimensions() {
        assert!(ShmBuffer::new(0, 64).is_err());
        assert!(ShmBuffer::new(64, 0).is_err());
        assert!(ShmBuffer::with_stride(64, 64, 128).is_err());
        assert!(ShmBuffer::with_stride(64, 64, 255).is_err());
    }

    #[test]
    fn from_fd_rejects_size_mismatch() {
        let buf = ShmBuffer::new(64, 64).unwrap();
        let dup = buf.as_fd().try_clone_to_owned().unwrap();
        // Stride dichiarata diversa da quella reale: la dimensione non torna.
        assert!(ShmBuffer::from_fd(dup, 64, 64, 512).is_err());
    }
}
