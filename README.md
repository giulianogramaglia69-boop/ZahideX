# ZahideX

Server grafico + compositore di finestre **da zero, tutto in Rust**, con protocollo nostro.
Non Wayland, non X11.

Target: laptop Fedora con GPU AMD, lanciato da TTY via logind (niente root).

## Stato

- [x] **Fase 1 — Protocollo v0.1 + IPC** (`zahidex-protocol`, `zahidex-ipc`)
- [x] **Fase 2 — Server DRM che accende lo schermo e disegna uno sfondo**
      (`zahidex-server`, binario `zahidex`; 8 test: scelta modo video,
      handshake)
- [ ] Fase 3 — Input libinput + focus
- [ ] Fase 4 — Compositor (tiling + floating)
- [ ] Fase 5 — Client library + client demo

## Architettura

```text
ZahideX/
├── crates/
│   ├── zahidex-protocol/   # SOLO tipi + (de)serializzazione. Nessuna dipendenza di sistema.
│   │                       # ClientMessage / ServerMessage, handshake, versioni, framing.
│   ├── zahidex-ipc/        # Trasporto: Unix socket, framing, SCM_RIGHTS, buffer memfd.
│   ├── zahidex-server/     # (Fase 2+) binario server: DRM, input, compositor, renderer.
│   └── zahidex-client/     # (Fase 5+) libreria client + client demo.
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
