<div align="center">

# anime-levisione

**Lo trovi. Scegli l'audio. Premi play.**

[![CI](https://github.com/k33wee/anime-levisione/actions/workflows/ci.yml/badge.svg)](https://github.com/k33wee/anime-levisione/actions/workflows/ci.yml)
![Rust 1.88+](https://img.shields.io/badge/rust-1.88%2B-e7a875?logo=rust&logoColor=white)
![Versione](https://img.shields.io/badge/versione-0.2.0-73e2a7)
[![GPL-3.0-only](https://img.shields.io/badge/licenza-GPL--3.0--only-73e2a7)](https://www.gnu.org/licenses/gpl-3.0.html)

[Italiano](README.it.md) · [English](README.md)

</div>

anime-levisione è un browser anime guidato dalla tastiera, fatto come lazygit: l'interfaccia non ti intralcia e tutto è a un tasto di distanza. Cerca contemporaneamente su AnimeWorld, AnimeUnity e StreamingCommunity, ricorda dove ti eri fermato e passa la riproduzione a VLC o mpv con la traccia audio che hai scelto.

I provider sono italiani: la maggior parte dei titoli ha i sottotitoli in italiano e molti anche il doppiaggio. Cambiano dominio di continuo — quando succede, anime-levisione va a cercarsi il nuovo indirizzo da solo e torna a funzionare.

Dalla 0.2.0 lo stesso cuore alimenta anche l'app Android: home con locandine, download offline e un player che chiede in che lingua vuoi sentire l'episodio *prima* di partire, completo di schermo intero.

| Terminale | Android |
| :---: | :---: |
| ![TUI: ricerca, dettagli, episodi, watchlist](docs/screenshots/library.png) | <img src="docs/screenshots/android-home.png" width="230" alt="Home Android con file di locandine"> <img src="docs/screenshots/android-player-audio.png" width="230" alt="Selettore audio pre-riproduzione"> <img src="docs/screenshots/android-fullscreen.png" width="230" alt="Player a schermo intero in orizzontale"> |

## Installazione (terminale)

Servono **Rust 1.88+** e **VLC o mpv**. Aggiungi `ffmpeg` per scaricare; `ffprobe` è facoltativo e serve solo a riconoscere le lingue dei file locali. Linux, macOS e Windows/WSL.

```sh
git clone https://github.com/k33wee/anime-levisione
cd anime-levisione
cargo install --path crates/tui --locked
```

Poi:

```sh
anime-levisione "frieren"       # cerca e sfoglia
anime-levisione --demo          # anteprima offline, niente rete né salvataggi
anime-levisione --player mpv    # scegli il player
anime-levisione --private       # niente AniList né cronologia automatica
anime-levisione doctor          # controlla player, config e provider
```

Tutto entra da **60×18** in su; le finestre più piccole ingrandiscono il pannello attivo.

| Tasto | Azione |
| --- | --- |
| `/` | Cerca |
| `Tab` / `Shift+Tab` · `0`–`7` | Cambia / vai al pannello |
| `j/k` · frecce | Naviga |
| `Enter` · `s` · `n` | Episodi / ricomincia / episodio successivo |
| `w` · `m` · `d` | Watchlist · segna visto · scarica |
| `Ctrl+x` · `Ctrl+d` | Ferma la riproduzione · annulla il download |
| `?` · `:` · `q` | Scorciatoie · palette comandi · esci |

Anche il mouse funziona: clic per attivare un pannello o scegliere una riga, rotella per scorrere.

**Audio.** Se un episodio ha più lingue, prima di partire compare un selettore: scegli con le frecce e conferma con `Enter`. Per gli HLS non serve nulla, gli altri file usano `ffprobe` se c'è, e in mancanza di metadata vale il default del player.

**AniList, solo se vuoi.** [Crea un client OAuth](https://anilist.co/settings/developer), poi `anime-levisione login --client-id IL_TUO_ID` — oppure accedi dalla palette comandi. La sincronizzazione porta fuori watchlist e progressi ma non degrada mai ciò che è già su AniList. `logout` scollega; con `--private` l'app non parla mai con AniList e non scrive cronologia.

**File.** `,` mostra dove stanno i dati. In `config.toml` trovi player, argomenti, cartella download e mirror dei provider; `--data-dir` usa una libreria separata.

## Android

<p>
<img src="docs/screenshots/android-home.png" width="180" alt="Home">
<img src="docs/screenshots/android-detail.png" width="180" alt="Dettagli ed episodi">
<img src="docs/screenshots/android-player-controls.png" width="180" alt="Controlli del player">
<img src="docs/screenshots/android-fullscreen.png" width="180" alt="Player a schermo intero">
<img src="docs/screenshots/android-settings.png" width="180" alt="Impostazioni">
</p>

Stessi provider e stesso formato libreria dell'app da terminale, dentro un'interfaccia scura costruita intorno alle locandine:

- Home con titolo in evidenza, riprendi a guardare e file per provider; ricerca con filtri
- My List, punti di ripresa, episodi visti, cronologia
- Player con scelta dell'audio prima della partenza, sottotitoli, limite qualità, schermo intero, episodio successivo automatico
- Download offline (HLS/MP4 nativi, solo Wi-Fi di default) che sopravvivono ai link scaduti
- Metadati, accesso e sincronizzazione AniList; modalità privata; mirror dei provider e diagnostica

Si compila da sorgente — la guida completa è in [android/README.md](android/README.md):

```sh
./android/build-rust-android.sh
./android/gradlew -p android :app:assembleDebug
adb install -r android/app/build/outputs/apk/debug/app-debug.apk
```

La CI allega a ogni run su `main` un APK e un AAB appena compilati. Non è sul Play Store: è un client basato su scraping, quindi solo installazione manuale.

## Sviluppo

```sh
make help        # tutti i target
make ci          # fmt + clippy + test + commitlint (da lanciare prima del push)
make bindings    # rigenera i binding Kotlin dopo modifiche alla core API
make hooks       # hook commit-msg / pre-push
make act         # ripete la workflow GitHub in locale (serve Docker)
```

I commit seguono i [Conventional Commits](https://www.conventionalcommits.org/it) e vengono verificati da commitlint in CI. Il workspace è diviso in tre crate: `crates/core` (provider, AniList, libreria — tutto ciò che è portabile e condiviso con Android via UniFFI), `crates/tui` (l'app da terminale) e `crates/bindgen` (il generatore dei binding Kotlin). La CI esegue la suite Rust su Linux, macOS e Windows, e compila più smoke-testa l'app Android su emulatori con Android 8 e 15.

## Licenza

GPL-3.0-only. Il progetto non è affiliato ad AnimeWorld, AnimeUnity, StreamingCommunity o AniList e non distribuisce contenuti: è un player per servizi che usi già, quindi niente garanzia e un po' di buonsenso.
