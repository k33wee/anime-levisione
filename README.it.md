<div align="center">

<img src="docs/branding/icon.svg" width="96" alt="Logo anime-levisione">

# anime-levisione

[![CI](https://github.com/k33wee/anime-levisione/actions/workflows/ci.yml/badge.svg)](https://github.com/k33wee/anime-levisione/actions/workflows/ci.yml)
![Versione](https://img.shields.io/badge/versione-0.4.1-73e2a7)
[![GPL-3.0-only](https://img.shields.io/badge/licenza-GPL--3.0--only-73e2a7)](https://www.gnu.org/licenses/gpl-3.0.html)

[English](README.md) · Italiano

</div>

Un browser anime per **terminale desktop** e **Android**. Entrambi i client cercano
su AnimeWorld, AnimeUnity e StreamingCommunity, salvano watchlist e progressi e
permettono di scegliere la lingua audio prima della riproduzione.

I provider offrono soprattutto contenuti doppiati o sottotitolati in italiano.
Cambiano dominio spesso: il core Rust condiviso cerca indirizzi alternativi
quando uno smette di funzionare.

**[TUI](#tui)** · **[App Android](#app-android)** · [Sviluppo](#sviluppo)

---

## TUI

Il client desktop: interfaccia a pannelli, scorciatoie da tastiera e supporto
mouse. VLC o mpv riproduce il video; il terminale resta disponibile per sfogliare
il catalogo.

![TUI: ricerca, dettagli, episodi e watchlist](docs/screenshots/library.png)

*Catalogo dimostrativo offline: i titoli e le descrizioni mostrati sono inventati.*

### Installazione e avvio

Servono **Rust 1.88+** e **VLC o mpv**. Installa `ffmpeg` per i download;
`ffprobe` è facoltativo per rilevare le tracce audio nei formati diversi da HLS.
Funziona su Linux, macOS e Windows/WSL. Gli archivi di release non includono
un player multimediale.

Su Windows, VLC viene cercato nelle cartelle Program Files standard, accanto ad
`anime-levisione.exe` e nel `PATH`. Per un'installazione personalizzata usa, ad
esempio, `anime-levisione.exe --player "C:/percorso/di/vlc.exe"`.

```sh
git clone https://github.com/k33wee/anime-levisione
cd anime-levisione
cargo install --path crates/tui --locked

anime-levisione "frieren"
```

Altre opzioni di avvio:

```sh
anime-levisione --demo          # anteprima offline, senza rete né salvataggi
anime-levisione --player mpv    # scegli il player
anime-levisione --private       # disabilita sync e cronologia automatica
anime-levisione doctor          # controlla player, configurazione e provider
```

### Comandi

| Tasto | Azione |
| --- | --- |
| `/` | Cerca |
| `Tab` / `Shift+Tab`, `0`–`7` | Cambia pannello o vai direttamente a uno |
| `j/k`, frecce | Naviga |
| `Enter` | Apri un titolo / riproduci l'episodio selezionato |
| `s`, `n`, `N` | Ricomincia / episodio successivo / precedente |
| `w`, `m`, `d` | Watchlist / segna visto / scarica |
| `Ctrl+x`, `Ctrl+d` | Ferma la riproduzione / annulla il download |
| `?`, `:`, `q` | Scorciatoie / palette comandi / esci |

Clic per attivare un pannello o selezionare una riga, rotella per scorrere.
Il layout entra da 60×18; le finestre più piccole mostrano il pannello attivo.
Se sono disponibili più lingue audio, scegli con le frecce e conferma con `Enter`.

### Libreria e sincronizzazione Telegram personale

Watchlist, cronologia e punti di ripresa vengono salvati in locale. Premi `,`
per vedere i percorsi dei dati; `config.toml` contiene player, argomenti,
cartella download e mirror dei provider. `--data-dir` usa una libreria separata.

La sincronizzazione facoltativa usa **un tuo bot Telegram dedicato**, senza
account dell'app né server del progetto. Crea il bot tramite @BotFather, invia
`/start` nella sua chat e collegalo dalla palette comandi oppure:

```sh
anime-levisione login     # primo dispositivo: incolla il token nel prompt nascosto
anime-levisione sync      # sincronizza liste e progressi in entrambe le direzioni
anime-levisione account   # mostra la chiave SEGRETA per gli altri dispositivi
anime-levisione logout    # scollega soltanto, conserva entrambe le librerie
```

Watchlist, visti/non visti e punti di ripresa vengono cifrati e salvati in un
documento fissato nella chat del bot. I dispositivi possono sincronizzarsi in
momenti diversi. I titoli sono identificati dagli ID dei provider, non dal nome.
La modalità privata blocca sync e salvataggi automatici di cronologia e ricerche,
non i dati esistenti o le modifiche locali esplicite.

#### Collegare un'altra TUI o un APK

La sync funziona **TUI ↔ TUI, TUI ↔ APK e APK ↔ APK**. Il primo dispositivo può
usare uno qualsiasi dei due client; quelli della stessa libreria devono usare
**lo stesso bot e la chiave completa `alib1.…`**. Non creare un bot per ogni
dispositivo.

1. Completa la prima sincronizzazione e recupera la chiave con **Show secret
   connection key** nell'app/TUI oppure con `anime-levisione account`.
2. Sull'altra TUI esegui `anime-levisione login` o scegli **Connect personal
   Telegram bot** dalla palette. Incolla la **chiave di connessione**, non il solo
   token di BotFather. Su Android usa Settings → Personal Telegram sync.
3. Le modifiche di ciascun client arrivano agli altri alla sync successiva, anche
   se i dispositivi non sono mai online insieme. Con i client aperti, la sync
   avviene circa ogni minuto; puoi richiederla subito con **Sync library now**
   nella TUI o **Sync library** nell'APK.

Chiudi la TUI prima di eseguire i comandi CLI account/sync sulla stessa cartella
dati, oppure usa la sua palette. Installa una build con questa integrazione su
**ogni** dispositivo: le vecchie release con il solo AniList non possono
collegarsi. Conserva privatamente una copia della chiave: perdendo sia questa
sia tutti i profili locali, la libreria cifrata non è recuperabile.

[Configurazione, recupero e limiti delle scritture simultanee →](docs/telegram-sync.md)

---

## App Android

Il client mobile usa Compose e Media3: non servono player esterni, `ffmpeg` o
`ffprobe`. Richiede **Android 8.0 o successivo**.

<p>
<img src="docs/screenshots/android-home.png" width="210" alt="Home Android">
<img src="docs/screenshots/android-detail.png" width="210" alt="Dettagli del titolo ed episodi">
<img src="docs/screenshots/android-player-audio.png" width="210" alt="Scelta della lingua audio">
</p>

*Gli screenshot usano [illustrazioni originali e video sintetici](docs/screenshots/README.md),
non locandine o fotogrammi dei provider.*

- **Catalogo:** titolo in evidenza, righe per provider, filtri di ricerca e My List.
- **Visione:** punti di ripresa, episodi visti, cronologia ed episodio successivo
  automatico, se abilitato.
- **Player:** scelta audio prima della partenza, sottotitoli, limite qualità e
  schermo intero orizzontale. Tocca il video per play/pausa, avanzamento e altri
  comandi: durante la riproduzione si nascondono, senza rimpicciolire il video.
- **Download:** episodi HLS/MP4 con preferenze audio e qualità, solo Wi-Fi di
  default. I download completati funzionano dalla cache dell'app anche se il
  link originale scade. Si possono sospendere, riprendere e rimuovere da Downloads.
- **Impostazioni:** sincronizzazione Telegram personale, modalità privata,
  mirror dei provider e diagnostica della connessione.

<img src="docs/screenshots/android-fullscreen.png" width="680" alt="Player a schermo intero orizzontale">

### Scaricare l'app

I pacchetti sono allegati alle [release](https://github.com/k33wee/anime-levisione/releases/latest):

- `anime-levisione-<versione>-android.apk` — pacchetto Android firmato (Android 8+)
- `anime-levisione-<versione>-<piattaforma>.tar.gz|.zip` — binari TUI per Linux
  (glibc 2.35+), macOS (Apple Silicon e Intel) e Windows x86_64
- `SHA256SUMS` — checksum di tutti gli asset

Scarica l'APK sul telefono e aprilo, oppure installa con ADB:

```sh
adb install anime-levisione-<versione>-android.apk
```

L'APK di release è firmato con la chiave di release del progetto. Le build di
debug (artefatti CI, `assembleDebug`) usano una chiave diversa: Android rifiuta
l'aggiornamento dall'una all'altra e la disinstallazione cancella libreria e
download locali. Usa l'APK di release.

<details>
<summary>Alternative</summary>

- APK e AAB di debug da una qualsiasi
  [run CI riuscita su main](https://github.com/k33wee/anime-levisione/actions/workflows/ci.yml?query=branch%3Amain)
  (artefatto `android-debug`, serve l'accesso a GitHub). L'AAB non si installa
  direttamente.
- Compilare dai sorgenti — configura Rust, JDK 17+, SDK Android e NDK come
  descritto nella [guida di compilazione Android](android/README.md#build), poi:

```sh
./android/build-rust-android.sh
./android/gradlew -p android :app:assembleDebug
adb install -r android/app/build/outputs/apk/debug/app-debug.apk
```

</details>

### Libreria e account

Download, libreria e credenziali del bot restano nello spazio privato dell'app.
Rimuovere un download conserva i progressi; disinstallare elimina i dati locali.
La sync Telegram facoltativa conserva una copia cifrata delle liste e dei punti
di ripresa, non dei video.

In Settings → Personal Telegram sync collega il nuovo bot oppure incolla la
chiave segreta `alib1.…` del primo dispositivo. Conservala prima di disinstallare.
La sincronizzazione funziona con il client aperto: Telegram non è un database
transazionale, quindi eventuali scritture simultanee convergono quando i
dispositivi coinvolti si sincronizzano nuovamente.
[Configurazione completa e limiti →](docs/telegram-sync.md)

[Configurazione, test e dettagli sullo spazio dati Android →](android/README.md)

---

## Log diagnostici

Dalla 0.4.1 entrambi i client conservano log locali rotanti: quattro file da
massimo 256 KiB ciascuno. Non viene inviato nulla automaticamente. Si registrano
versione app/OS, architettura, timestamp, nomi dei provider, ID tecnici delle
operazioni, codici HTTP, retry/cancellazioni e tempi/codici errore del player.
Sono esclusi titoli, ricerche, cronologia e posizioni di visione, ID dei contenuti,
URL, percorsi, messaggi d'errore grezzi, token Telegram e connection key.
La diagnostica senza contenuti resta attiva in modalità privata; `--demo` non
scrive nulla.

- **TUI:** riproduci il problema, poi esegui
  `anime-levisione logs > anime-levisione-diagnostics.jsonl` e allega il file
  all'issue. Se personalizzato, usa lo stesso `--data-dir`. L'esportazione funziona
  anche con libreria corrotta o TUI aperta (snapshot best-effort durante la
  rotazione). I file sono in `logs/` nella cartella dati mostrata premendo `,`.
- **Android:** **Settings → Providers & support → Export diagnostic logs**.
  Conferma e scegli l'app a cui condividere lo snapshot. L'accesso temporaneo in
  lettura riguarda solo il file diagnostico, mai libreria o download. La cache
  conserva al massimo tre snapshot esportati; Android può eliminarli.

Allega anche passaggi per riprodurre, versione e sistema operativo. I log non
ricostruiscono problemi delle vecchie versioni: riproducili dopo l'aggiornamento.
Panic Rust ed eccezioni Android non gestite registrano soltanto categorie sicure
e numeri di riga, non dump completi. Problemi di storage/logging non bloccano l'app.

## Sviluppo

```sh
make help        # target disponibili
make ci          # fmt, clippy, test e commitlint; da eseguire prima del push
make bindings    # rigenera i binding Kotlin dopo modifiche alla core API
make hooks       # abilita gli hook commit-msg e pre-push
make act         # esegue il workflow GitHub in locale; richiede Docker
```

| Percorso | Contenuto |
| --- | --- |
| `crates/core` | Provider, libreria e sync Telegram cifrata; condivisi tramite UniFFI |
| `crates/tui` | Applicazione da terminale |
| `crates/bindgen` | Generatore dei binding Kotlin |
| `android` | Applicazione Android e script di compilazione |
| `docs/branding` | [Icona modificabile e generazione delle risorse](docs/branding/README.md) |

La CI verifica Rust su Linux, macOS e Windows, controlla i binding generati e
compila, analizza e testa Android su emulatori API 26 e 35. I commit seguono i
[Conventional Commits](https://www.conventionalcommits.org/it).

## Licenza

GPL-3.0-only. Non vengono distribuiti contenuti. Il progetto non è affiliato ai
provider, a Telegram o a Rai/Melevisione. La disponibilità dipende dai servizi
originali; usali nel rispetto dei diritti e delle condizioni applicabili.
