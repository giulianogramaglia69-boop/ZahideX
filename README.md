# ZahideX

Server grafico + compositore di finestre **da zero, tutto in Rust**, con protocollo nostro.
Non Wayland, non X11.

Target: laptop Fedora con GPU AMD, lanciato da TTY via logind (niente root).

## Stato

- [x] **Fase 1 — Protocollo v0.1 + IPC** (`zahidex-protocol`, `zahidex-ipc`)
- [x] **Fase 2 — Server DRM che accende lo schermo e disegna uno sfondo**
      (`zahidex-server`, binario `zahidex`; 8 test: scelta modo video,
      handshake)
- [x] **Fase 3 — Input libinput + focus** (`zahidex-server`: tastiera
      xkb italiana, pointer/touch, cursore software, focus al primo client;
      16 test nuovi)
- [x] **Fase 4 — Compositor (tiling + floating)** (`zahidex-server`:
      registro finestre, layout tiling master 60% + stack, floating con
      drag della barra del titolo e resize dai bordi, focus per-finestra,
      scorciatoie Super+J/K/F/Q, rendering con barre del titolo e bordo
      di focus; client di test `zahidex-test-client`; 22 test nuovi)
- [x] **Fase 5 — Client library + client demo** (`zahidex-client`:
      `Client`/`Window`, handshake, buffer memfd, eventi con `Pong`
      automatico; demo `zahidex-demo` con gradiente e quadrato
      interattivo; client di test migrato sopra la libreria; 7 test
      di integrazione contro server finto + 1 test di disegno)

## Architettura

```text
ZahideX/
├── crates/
│   ├── zahidex-protocol/   # SOLO tipi + (de)serializzazione. Nessuna dipendenza di sistema.
│   │                       # ClientMessage / ServerMessage, handshake, versioni, framing.
│   ├── zahidex-ipc/        # Trasporto: Unix socket, framing, SCM_RIGHTS, buffer memfd.
│   ├── zahidex-server/     # (Fase 2+) binario server: DRM, input, compositor, renderer.
│   └── zahidex-client/     # (Fase 5) libreria client + client demo.
└── README.md
```

La ricerca sull'ecosistema Rust (DRM, wgpu, libinput, libseat, IPC) sta in
`../nuovo-server-grafico/RICERCA_ECOSISTEMA.md`.

## Fase 2 — Server DRM (fatta)

Il binario `zahidex` (`crates/zahidex-server/`) accende lo schermo e
disegna uno sfondo a colore pieno.

- **Base: Smithay 0.7.0** (solo basso livello: DRM, GBM, EGL, sessione
  libseat, udev). Il protocollo resta il nostro. Smithay 0.7 usa
  **drm 0.14** (non 0.15: la 0.15 non è compatibile con i tipi di Smithay
  0.7). La feature `wayland_frontend` di Smithay è attiva solo perché in
  0.7 il modulo `backend::drm::compositor` (`DrmCompositor`) è compilato
  solo con quella feature — **nessun codice Wayland viene usato**.
- **Avvio da TTY, niente root**: sessione libseat (`seat0` di default) —
  logind concede i device e la VT.
- **Modeset** alla risoluzione nativa: il primo connettore collegato con
  modi, CRTC dall'encoder (o il primo disponibile), modo preferito (se c'è)
  altrimenti il più grande, poi il refresh più alto.
- **Renderer: GlesRenderer su GBM** (pattern anvil/Niri): `EGLDisplay` +
  `EGLContext` sul `GbmDevice`, `DrmCompositor` con allocator GBM
  (RENDERING | SCANOUT). Lo sfondo è un **colore pieno via clear GPU**:
  blu scuro **`#0B1D33`** (RGB 11, 29, 51) — sobrio, scuro, non affatica.
  **tiny-skia NON entra in questa fase**: la rasterizzazione software
  serve solo quando disegneremo contenuti veri (finestre, testo); per un
  colore pieno il clear GPU è più semplice e più veloce.
- **Page flip**: il frame è accodato con `queue_frame` e confermato al
  VBlank (`frame_submitted`). Lo sfondo è ridisegnato solo all'avvio e
  dopo un cambio VT: il resto del tempo il loop dorme.
- **VT switch**: libseat notifica pausa/riattivazione — il server rilascia
  il DRM master in pausa e lo riacquisisce al ritorno, ridisegnando lo
  sfondo.
- **Uscita pulita**: Ctrl+C / SIGTERM fermano il loop calloop; i `Drop`
  rilasciano DRM master e sessione, logind ripristina la TTY precedente
  (niente schermo nero bloccato). Se la GPU viene rimossa (udev), il
  server si spegne da solo.
- **IPC**: il server accetta il socket della Fase 1
  (`$XDG_RUNTIME_DIR/zahidex-0`) e risponde solo all'handshake
  (`Hello` → `HelloOk` con la dimensione dell'output, o `HelloError`);
  gli altri messaggi vengono registrati nel log e ignorati — la gestione
  finestre è Fase 4.
- **Log su stdout con timestamp** (`tracing`, livello via `RUST_LOG`,
  default `info`).

### Avvio sul laptop (da TTY)

```sh
# 1. Compila (serve Rust 1.88+ e le librerie di sviluppo: libseat-dev,
#    libgbm-dev, libegl-dev, libudev-dev, libinput-dev, libwayland-dev)
cargo build --release -p zahidex-server

# 2. Esci da GNOME (o vai su una TTY libera: Ctrl+Alt+F3) e loggati

# 3. Controllo prerequisiti (non accende niente, esce sempre 0):
./target/release/zahidex --dry-run

# 4. Avvia: lo schermo diventa blu scuro #0B1D33
./target/release/zahidex

# 5. Per uscire: Ctrl+C — torni alla TTY, niente schermo nero.
#    Log: vanno su stdout; per salvarli:
./target/release/zahidex 2>&1 | tee ~/zahidex.log
```

Note: non avviare da SSH né da dentro un altro ambiente grafico — serve
una sessione logind attiva sulla TTY. Se il server si lamenta di
`NoGpu`, controlla che la sessione sia attiva (`loginctl`) e che il
seat sia quello giusto (`--seat seat0`).

## Fase 3 — Input libinput + focus (fatta)

Input reale da tastiera, mouse e touch, con focus al client connesso.

- **Backend libinput di Smithay 0.7** (`backend::libinput`), integrato nel
  loop calloop esistente come sorgente eventi. La sessione libseat apre i
  device `/dev/input/event*` con i permessi logind (niente root).
  **Hotplug**: aggiunta/rimozione a caldo via `DeviceAdded`/`DeviceRemoved`
  di libinput (log con nome e sysname del device).
- **Deviazione dalle versioni della ricerca**: Smithay 0.7 fissa
  **`input` 0.9.0** e **`xkbcommon` 0.8.0** (non 0.10 / 0.9 come indicato
  in `RICERCA_ECOSISTEMA.md`): `LibinputInputBackend::new` prende
  l'`input` 0.9 e Smithay dipende direttamente da `xkbcommon` 0.8.0, le
  versioni nuove non coesistono con i suoi tipi. Stessa regola di Smithay
  0.7 → drm 0.14: le versioni le detta la base, non la ricerca.
- **Tastiera con xkbcommon**: keycode evdev grezzi (xkb − 8) tradotti con
  layout **`it`** di default (flag `--layout <nome>`, es. `--layout us`).
  Ogni evento `Key` porta `keycode` (evdev), `keysym` (u32 xkb) e
  `modifiers` (bitmask: SHIFT=1, CTRL=2, ALT=4, SUPER=8).
- **Pointer**: motion relativo (mouse) e assoluto (touch/tablet, mappato
  sulle dimensioni dell'output), bottoni (codici evdev, es. BTN_LEFT
  0x110), scroll (`PointerAxis`: `delta_v120` per le rotelle, `delta_px`
  per lo scroll continuo/dita — esattamente uno dei due è impostato).
  **Touch**: tap = movimento assoluto + pressione/rilascio BTN_LEFT;
  gli eventi touch multi-dito oltre il primo sono ignorati in Fase 3.
- **Cursore software**: freccia 16×16 (bianca con bordo nero, disegnata in
  CPU come `MemoryBuffer` ABGR8888) renderizzata da GlesRenderer sopra lo
  sfondo; parte al centro dell'output, si muove con il mouse, si
  riposiziona con il touch e resta bloccata ai bordi dell'output.
- **Focus (modello Fase 3, senza finestre)**: il focus è "quale client
  riceve l'input". Il **primo client connesso è focalizzato** e riceve
  tutti gli eventi tastiera (`Key`); quando si disconnette, il focus passa
  al prossimo client connesso e il nuovo focalizzato riceve `Focus {
  window_id: None }` (la sentinella `WindowId(0)` significa "livello
  output, nessuna finestra" — le finestre sono Fase 4). Se nessun client è
  connesso, gli eventi tastiera vanno solo nel log; gli eventi pointer
  aggiornano comunque il cursore. I client non focalizzati non ricevono
  eventi di input.
- **IPC**: i thread client ora svuotano la coda dei messaggi in uscita
  mentre aspettano il prossimo messaggio, con `wait_readable` (poll, 20 ms)
  invece di un timeout di lettura che potrebbe scattare a metà frame.
- **Scorciatoie di uscita** (uscita pulita come Ctrl+C/SIGTERM):
  **Ctrl+Alt+BackSpace** oppure **Super+Q**. Il server le intercetta
  prima di inoltrarle al client e si spegne rilasciando DRM master e
  sessione.

### Protocollo v0.1 — cambiamenti della Fase 3

Tutti compatibili (campi nuovi solo in coda, varianti nuove solo in coda):

- `Key` ha ora `keysym: u32` e `modifiers: u32` (in coda ai campi Fase 1).
- `ServerMessage` ha la nuova variante `PointerAxis { window_id, axis,
  delta_v120, delta_px }` (in coda).
- `modifiers` (modulo in `zahidex-protocol`): costanti SHIFT=1, CTRL=2,
  ALT=4, SUPER=8.
- `ServerMessage` deriva solo `PartialEq` (non più `Eq`): `delta_px` è
  `f64` e `f64` non implementa `Eq`.

### Provare tastiera e mouse sul laptop (Fase 3)

```sh
# 1. Compila e lancia da una TTY libera (come per la Fase 2):
./target/release/zahidex --dry-run   # elenca anche i dispositivi input visti
./target/release/zahidex

# 2. Tastiera: ogni tasto premuto va nel log (RUST_LOG=debug mostra anche
#    keysym e modificatori). Prova: lettere, Shift+lettera (maiuscola),
#    Ctrl, Alt, Super — il log mostra la maschera modificatori.
RUST_LOG=debug ./target/release/zahidex

# 3. Mouse: muovi il mouse — la freccia bianca si sposta sullo sfondo blu.
#    Rotella: lo scroll arriva come evento PointerAxis (log, nessun effetto
#    visivo in Fase 3). Click: log del bottone (nessun effetto in Fase 3:
#    non ci sono finestre da cliccare).

# 4. Touch (se il laptop lo ha): tocca lo schermo — il cursore salta al
#    punto toccato.

# 5. Uscita: Ctrl+Alt+BackSpace oppure Super+Q (oppure Ctrl+C).
#    Il server si spegne pulito e torni alla TTY.

# 6. Hotplug: collega/scollega mouse o tastiera USB mentre il server gira —
#    il log registra "dispositivo input aggiunto/rimosso" e il nuovo device
#    funziona subito, senza riavvio.
```

Senza client connessi gli eventi tastiera finiscono solo nel log: per
vederli arrivare a un client serve la libreria client della Fase 5 (o un
client di test che fa handshake sul socket `$XDG_RUNTIME_DIR/zahidex-0`).
Il cursore invece si muove anche senza client.

## Fase 4 — Compositor (fatta)

`crates/zahidex-server/src/compositor.rs`: logica pura (niente DRM, niente
I/O) — registro finestre, layout, focus, drag, hit-test. Il rendering vive
in `backend.rs`, lo smistamento dei messaggi in `ipc.rs` / `input.rs`.

- **Layout tiling (default)**: master a sinistra (60% della larghezza) +
  stack a destra diviso in parti uguali in verticale; una sola finestra
  occupa tutto l'output. Il layout si rifà a ogni creazione, distruzione e
  cambio floating; il server invia `Configure` ai client la cui geometria
  cambia.
- **Floating**: `SetFloating=true` (o Super+F) sposta la finestra sopra le
  tiled; posizione iniziale centrata, dimensione iniziale 800×600 (o metà
  output se più piccolo) — in v0.1 la dimensione richiesta in
  `CreateWindow` è quella iniziale. Drag dalla barra del titolo per
  spostarla, resize dai bordi destro/inferiore e dall'angolo in basso a
  destra (8 px). La floating focalizzata sale in cima allo z-order.
- **Rendering**: buffer dei client (texture da memfd XRGB8888) sopra lo
  sfondo #0B1D33, in ordine z (tiled sotto, floating sopra, focus in cima
  tra le floating); barra del titolo 24 px (blu #2563EB con focus, grigia
  #334155 senza — senza testo per ora); bordo di 2 px #0A84FF sulla
  finestra con focus; segnaposto scuro #16283D se la finestra non ha ancora
  committato un buffer. Ridisegno a ogni commit, focus, layout, movimento
  cursore o drag.
- **Focus per-finestra**: click su una finestra → focus a lei (hit-test:
  floating sopra tiled, barra del titolo inclusa); la tastiera va alla
  finestra con focus; il pointer va alla finestra sotto il cursore
  (coordinate relative all'origine della finestra). Super+J / Super+K
  ciclano il focus; Super+F passa la finestra con focus da tiling a
  floating e viceversa; Super+Q chiude la finestra con focus (se non c'è
  nessuna finestra, chiude il server come in Fase 3; Ctrl+Alt+BackSpace
  esce sempre).
- **Protocollo**: `TITLE_BAR_HEIGHT = 24` ora è una costante pubblica del
  protocollo (`zahidex_protocol::TITLE_BAR_HEIGHT`): il client la sottrae
  dall'altezza della finestra per ottenere l'area di contenuto del buffer.

### Provare il compositor sul laptop (Fase 4)

```sh
# 1. Compila (serve Rust 1.88+ e le librerie di sviluppo: libseat-devel,
#    libgbm-devel, libEGL-devel, libudev-devel, libinput-devel, wayland-devel)
cargo build --release

# 2. Su una TTY libera (Ctrl+Alt+F3), lancia il server:
./target/release/zahidex

# 3. Da un'altra TTY (o da SSH, il client non tocca il video), lancia il
#    client di test: due finestre, una rossa e una verde.
#    Senza argomenti resta aperto finché non lo chiudi (Ctrl+C);
#    con un numero, esce da solo dopo quei secondi.
./target/release/zahidex-test-client
./target/release/zahidex-test-client 30
```

Cosa devi vedere:

1. La finestra rossa occupa il 60% sinistro dello schermo, la verde il 40%
   destro; entrambe hanno la barra del titolo in alto e la rossa (focus)
   ha il bordo blu.
2. Click sulla verde → il bordo blu passa a lei. Trascina la sua barra del
   titolo → non si muove (è in tiling).
3. Super+F sulla verde → diventa floating, centrata, 800×600 (o metà
   schermo se più piccolo); ora trascinala dalla barra del titolo e
   ridimensionala dal bordo destro/inferiore.
4. Super+J / Super+K → il focus cicla tra le due finestre (bordo blu).
5. Super+Q → chiude la finestra con focus; l'altra si allarga a tutto lo
   schermo. Super+Q di nuovo (nessuna finestra) → il server si spegne.
6. Nel terminale del client vedi i messaggi che arrivano: `Configure`,
   `Focus`, `PointerEnter/Leave/Motion`, `Key` (prova a scrivere con una
   finestra focalizzata).

## Fase 5 — Client library + client demo (fatta)

`crates/zahidex-client/`: l'API pubblica per scrivere client ZahideX
senza toccare il protocollo grezzo.

- **`Client::connect()`** — apre il socket `$XDG_RUNTIME_DIR/zahidex-0`
  (fallback `/tmp`), fa l'handshake con negoziazione versione e restituisce
  la versione negoziata e la dimensione dell'output
  (`Client::output_size()`). Varianti: `connect_as(nome)` e
  `connect_to(percorso, nome)`. Se il server rifiuta l'handshake,
  `ClientError::HandshakeRejected`; se non c'è nessun server,
  `ClientError::Io` ("No such file or directory").
- **`Client::create_window(titolo, larghezza, altezza, floating)`** —
  crea la finestra e attende l'ack del server (`WindowCreated`); con
  `floating = true` invia subito anche `SetFloating` e la geometria
  finale arriva con il `Configure` successivo.
- **`Window::commit(pixel)` / `Window::commit_with(disegna)`** — il
  contenuto è XRGB8888, largo `width` e alto `height - TITLE_BAR_HEIGHT`
  (la barra del titolo la disegna il server). Il buffer è un memfd creato
  e passato via SCM_RIGHTS a ogni commit; `commit` valida la dimensione
  dei pixel prima di inviare (`ClientError::InvalidPixels`).
- **`Window::set_title` / `set_floating` / `request_resize` /
  `destroy`** — gli altri messaggi del protocollo.
- **Eventi**: `Client::poll_event(timeout)` (un evento, `None` = attesa
  senza scadenza, `Some(Duration::ZERO)` = non bloccante) e
  `Client::poll_events(timeout)` (tutti quelli disponibili). Il tipo è
  `Event` (`Configure`, `Focus`, `PointerEnter/Leave/Motion/Button/Axis`,
  `Key`, `CloseWindow`, `ServerError`). Il `Ping` del server **non arriva
  mai all'applicazione**: la libreria risponde `Pong` da sola. Quando
  arriva un `Configure`, la geometria della `Window` è già aggiornata
  (`window.width()` / `window.height()`): basta ricommittare.
- **Errori tipizzati** (`ClientError`, thiserror): niente panic su input
  del server malformato; la chiusura del server diventa
  `ClientError::Disconnected`.
- Il client è **single-thread** (`Rc`/`RefCell` interni): per un altro
  thread, apri una connessione dedicata.

### Esempio minimo

```rust
use zahidex_client::{Client, Event};

let mut client = Client::connect()?;
let window = client.create_window("Ciao", 800, 600, false)?;
// Riempie il contenuto di rosso (XRGB8888: byte B, G, R, X).
window.commit_with(|pixels, _w, _h, _stride| {
    for px in pixels.as_chunks_mut::<4>().0 {
        px[0] = 0;
        px[1] = 0;
        px[2] = 255;
        px[3] = 255;
    }
})?;
loop {
    for event in client.poll_events(None)? {
        if let Event::CloseWindow { .. } = event {
            return Ok(());
        }
    }
}
```

### Il demo (`zahidex-demo`)

`crates/zahidex-client/examples/demo.rs`, binario `zahidex-demo`: una
finestra con gradiente di sfondo blu, un quadrato arancione e un pulsante
di chiusura rosso in alto a destra.

| Azione | Effetto |
| --- | --- |
| Frecce o WASD | Muovono il quadrato (24 px) |
| Click sinistro | Il quadrato salta sotto il cursore |
| Click sul pulsante rosso | Chiude la finestra ed esce |
| Q oppure Esc | Chiude la finestra ed esce |
| Resize dal tiling (es. Super+Q su un'altra finestra) | Il demo riceve `Configure` e ridisegna |

### Provare il demo sul laptop

```sh
# 1. Compila (serve Rust 1.88+ e le librerie di sviluppo, come per il server)
cargo build --release

# 2. Su una TTY libera (Ctrl+Alt+F3), lancia il server:
./target/release/zahidex

# 3. Da un'altra TTY (o da SSH, il client non tocca il video), lancia il demo:
./target/release/zahidex-demo

# 4. Prova: frecce/WASD, click, pulsante rosso, Q.
#    Per vedere il tiling in azione, lancia anche il client di test:
./target/release/zahidex-test-client
#    Le due finestre del test si affiancano al demo (master 60% + stack);
#    Super+J/K cicla il focus, Super+F rende floating, Super+Q chiude.
```

### Note

- **Bug trovato dai test e corretto in Fase 5** (`zahidex-ipc`,
  `Connection::wait_readable`): se il peer inviava dati e chiudeva subito
  dopo (`POLLIN` + `POLLHUP` insieme), la funzione restituiva errore
  invece dei dati — il client perdeva l'ultimo messaggio (es. `HelloOk` o
  `Key`). Ora `POLLIN` ha la precedenza: i dati arrivano, la chiusura si
  vede solo al poll successivo. Il fix vale anche per il lato server.
- Il client di test (`zahidex-test-client`, Fase 4) è stato **migrato
  sopra `zahidex-client`**: stesso comportamento (due finestre
  rossa/verde, ricommit su `Configure`, stampa eventi), ma niente più
  protocollo grezzo.
- **Test**: 7 test di integrazione contro un server finto in-process
  (handshake ok/rifiutato, server assente, `CreateWindow`+`CommitBuffer`
  con verifica dei pixel nel memfd, floating, `Ping`→`Pong` automatico,
  `Key`, pixel di dimensione sbagliata) + 1 test dei pixel del demo.
  Totale workspace: **73 test** (64 + 9 nuovi: 7 di integrazione, 1
  doctest dell'esempio minimo, 1 dei pixel del demo).

## Protocollo v0.1

- Serializzazione dei messaggi con `postcard` (il `bincode` originale è
  **vietato**: non mantenuto, RUSTSEC-2025-0141).
- Framing: `[magic "ZX01" (4 byte)] [lunghezza payload u32 LE] [payload postcard]`.
- Handshake con negoziazione versione: il client dichiara le versioni che
  supporta, il server risponde con quella scelta o rifiuta.
- I buffer delle finestre sono memfd condivisi, passati via SCM_RIGHTS;
  formato v0.1: XRGB8888, stride = `width * 4`.
- Versionamento: enum chiusi, campi nuovi solo in coda, mai rimuovere
  o rinumerare varianti esistenti.

## Sviluppo

```sh
cargo test --workspace
cargo-clippy clippy --workspace --all-targets -- -D warnings
```

Toolchain: Rust standalone in `~/.rust-standalone`
(`PATH=$HOME/.rust-standalone/bin:$PATH`).

## Licenza

GPLv3 — vedi [LICENSE](LICENSE). Tutto il codice di ZahideX è software libero.

## Sicurezza (v0.1)

Nessun isolamento tra client: un client può leggere le finestre degli altri
e sniffare l'input globale. Il socket vive in `$XDG_RUNTIME_DIR` (0700, solo
l'utente). La sandbox per-client è lavoro futuro, non un requisito della v0.1.
