# osu!backup

Backs up **osu!lazer** to Google Drive. It lives in the system tray, watches for the
osu! process and, when the game closes, uploads only what changed since the last run.

Backed up by default:

- `client.realm` — scores, collections, settings
- `skins/` and `exports/`
- `.ini` and `.json` files in the osu! data folder

Beatmaps and audio in `files/` are **not** uploaded; the game re-downloads them.

> Only osu!lazer is supported. The legacy osu! client is not.

## Download

Grab the latest build for your system from the
[Releases](../../releases) page:

- `osubackup-<version>-windows-x86_64.exe`
- `osubackup-<version>-linux-x86_64`

## First run

1. Create Google OAuth credentials (one time only):
   1. Create a project at <https://console.cloud.google.com>.
   2. Enable the **Google Drive API** (APIs & Services → Library).
   3. Configure the **OAuth consent screen** as *External* and click **Publish app**.
      In *Testing* mode, Google expires the login after 7 days.
   4. Go to **Credentials → Create credentials → OAuth client ID**, choose
      **Desktop app**, and download the JSON file.
2. Start osu!backup. It asks for the JSON file, opens your browser for Google
   sign-in, creates an `osu-lazer-backup` folder in your Drive, and asks for your
   osu!lazer data folder (the default is pre-selected).

When osu! closes, the backup runs and osu!backup exits on its own.

Default osu!lazer data folder:

- Windows: `%APPDATA%\osu`
- Linux: `~/.local/share/osu`

## Build from source

Rust stable is required.

Linux needs GTK 3 and the Ayatana AppIndicator library for the tray icon:

```sh
sudo apt install libgtk-3-dev libayatana-appindicator3-dev libxdo-dev
```

Then:

```sh
cargo build --release
cargo test
```

## Versioning and releases

Releases are fully automated by GitHub Actions (`.github/workflows/release.yml`).
Every push to `main` that contains new commits is released. The next version is
derived from the [Conventional Commits](https://www.conventionalcommits.org) since the
last `vX.Y.Z` tag:

| Commit                                  | Bump                     |
| --------------------------------------- | ------------------------ |
| `feat: …` / `feat(scope): …`            | minor (`0.1.0` → `0.2.0`) |
| `type!: …` or `BREAKING CHANGE:` footer | major (`0.2.0` → `1.0.0`) |
| anything else (`fix:`, `chore:`, …)     | patch (`0.1.0` → `0.1.1`) |

The release job creates the tag, builds the Windows and Linux binaries with that
version, and publishes them to a GitHub Release with auto-generated notes.
The version in `Cargo.toml` is stamped during the build and not committed back.

Before anything is built, the release workflow runs formatting, clippy and tests.
A failing check blocks the release. Pull requests and other branches run the same
checks on Windows and Linux (`.github/workflows/ci.yml`).

## Known limitations

- The refresh token is stored in `~/.config/osubackup/refresh_token` (mode `600` on Linux).
- Files are uploaded in a single request, with no resumable upload. Very large
  files may fail on unstable connections.
- osu!backup does not register itself to start with the system yet. Start it
  manually or add it to your startup apps so it can watch osu!.

## License

Released under the [MIT License](LICENSE).
