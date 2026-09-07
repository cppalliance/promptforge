# Generic Realtime STT installed-package acceptance

## Status

Accepted by the operator for the installed unsigned package built from current HEAD `2d1ecca8`. The applicable Gateway hashes match, and the operator's authoritative verdict for this build is `Works correctly. Accepted.` Prior attempts remain below as history and do not supersede this verdict.

- Acceptance gate: passed by authoritative operator verdict
- Current automated installed-package boundary: passed
- Current operator boundary: passed for the repaired live transcription and short-utterance Stop behavior recorded below
- Installed application: `C:\Users\Vinnie\AppData\Local\PromptForge\promptforge-workshop.exe`
- Current Workshop process ID: 68872
- Current Gateway process ID: 91128
- Signing: not tested
- Commit created: no

## Latest installed preparation from HEAD 2d1ecca8

### Source and prior process boundary

- Current HEAD: `2d1ecca839634034d5b70901229d9012e74a18a0`
- Current commit: `2d1ecca8` (`Reconcile explicitly skipped final ranges`)
- Installed Workshop or Gateway processes observed before rebuild: 0
- Installed Workshop or Gateway processes stopped: 0
- Installed Workshop or Gateway processes remaining before rebuild: 0
- Installed Workshop or Gateway processes observed immediately before installation: 0

### Release Gateway

- Command: `$env:RUSTUP_TOOLCHAIN='stable'; cargo build --release --locked -p gateway`
- Result: passed
- Summary: release profile finished in 21.80 seconds with 9 `gateway-stt` warnings
- Build started: `2026-09-07T11:38:03.1101939Z`
- Build finished: `2026-09-07T11:38:25.0377360Z`
- Artifact: `target/release/promptforge-gateway.exe`
- Last modified: `2026-09-07T11:38:24.5038009Z`
- Size: 14,536,192 bytes
- SHA-256: `2745D151F7ADD0368308D2029976A11D4BAF38ECBA262C0AC27F57E83D67F74B`

### Target-suffixed sidecar

- Command: `$triple='x86_64-pc-windows-msvc'; New-Item -ItemType Directory -Path 'crates\workshop\binaries' -Force | Out-Null; Copy-Item 'target\release\promptforge-gateway.exe' "crates\workshop\binaries\promptforge-gateway-$triple.exe" -Force`
- Artifact: `crates/workshop/binaries/promptforge-gateway-x86_64-pc-windows-msvc.exe`
- Last modified: `2026-09-07T11:38:24.5038009Z`
- Size: 14,536,192 bytes
- SHA-256: `2745D151F7ADD0368308D2029976A11D4BAF38ECBA262C0AC27F57E83D67F74B`
- Verification: source and staged SHA-256 hashes matched at `2026-09-07T11:38:39.6850080Z`

### Packaging tool

- Command: `$env:RUSTUP_TOOLCHAIN='stable'; cargo install tauri-cli --locked`
- Result: passed
- Installed version: `tauri-cli 2.11.4`
- Detail: Cargo reported that the same version was already installed
- Verified: `2026-09-07T11:38:39.6299436Z`

### Fresh unsigned local NSIS package

- Exact successful PowerShell command: `cargo --% tauri build --bundles nsis --config {\"bundle\":{\"createUpdaterArtifacts\":false}}`
- Working directory: `crates/workshop`
- Result: passed
- Build started: `2026-09-07T11:38:45.9576171Z`
- Build finished: `2026-09-07T11:39:54.3354945Z`
- Workshop release profile finished in 47.06 seconds
- Installer: `target/release/bundle/nsis/PromptForge_0.2.0_x64-setup.exe`
- Installer created: `2026-09-07T11:39:35.7152137Z`
- Installer last modified: `2026-09-07T11:39:54.2055172Z`
- Installer size: 12,393,556 bytes
- Installer SHA-256: `CE476DE44A6F7E0897765ED45AA6E988702826FC9F4B7083A155DBE90E90F028`
- Previous installer SHA-256: `DD2A21369B0834084F26D22ADAE92896431574506C607749F12FFC546ACB78D7`
- Freshness proof: the installer creation and modification timestamps follow the successful build start, and its hash differs from the previous installer
- Override scope: `bundle.createUpdaterArtifacts=false` was supplied only through the Tauri command line
- Protected release configuration: `crates/workshop/tauri.conf.json` and `.github/workflows/release-workshop.yml` have no diff
- Signing: not tested

The adjacent `PromptForge_0.2.0_x64-setup.exe.sig` remains stale from `2026-09-06T02:42:54.1030502Z` and is excluded from this build's evidence.

### Silent installation and installed identities

- Install result: passed
- Installer exit code: 0
- Install started: `2026-09-07T11:40:09.9664232Z`
- Install finished: `2026-09-07T11:40:13.3574707Z`
- Workshop path: `C:\Users\Vinnie\AppData\Local\PromptForge\promptforge-workshop.exe`
- Workshop file version: `0.2.0`
- Workshop product version: `0.2.0`
- Workshop last modified: `2026-09-07T11:39:34Z`
- Workshop size: 24,290,816 bytes
- Workshop SHA-256: `36AA10231DA4177C859494B2FD4A116C68EEA12B3BA7C704AB788D16AB6F532C`
- Build-tree Workshop size: 24,290,816 bytes
- Build-tree Workshop SHA-256: `41A5252E66179E48B76C9CED552B730EFF644F18788F87D8D4C6539EBEF1A867`
- Workshop comparison: both identities are recorded without claiming equality because the Tauri log records NSIS bundle-information patching during packaging
- Gateway sibling path: `C:\Users\Vinnie\AppData\Local\PromptForge\promptforge-gateway.exe`
- Gateway sibling last modified: `2026-09-07T11:38:24Z`
- Gateway sibling size: 14,536,192 bytes
- Gateway sibling SHA-256: `2745D151F7ADD0368308D2029976A11D4BAF38ECBA262C0AC27F57E83D67F74B`
- Gateway verification: installed, staged, and release SHA-256 hashes match
- Identity verification observed: `2026-09-07T11:40:29.6211886Z`

### Installed application launch

- Launched: `2026-09-07T11:40:35.8875527Z`
- Readiness-window observation: `2026-09-07T11:41:06.4335573Z`
- Workshop process ID: 68872
- Gateway process ID: 91128
- Both process paths resolve under `C:\Users\Vinnie\AppData\Local\PromptForge`
- Both processes remained running at `2026-09-07T11:41:37.3425936Z`
- No physical microphone, model-menu, or model-turn checklist item was observed during automated preparation

## Prior installed preparation after whole-window scheduler repair

### Source and prior process boundary

- Current HEAD: `006ba06d945ec0bfacbb0a0270f65d2706eb20db`
- Current commit: `006ba06d` (`Schedule and rebase whole-window hypotheses`)
- Installed Workshop or Gateway processes observed before rebuild: 0
- Installed Workshop or Gateway processes stopped: 0
- Installed Workshop or Gateway processes remaining before rebuild: 0
- Process boundary observed: `2026-09-07T10:42:20.0755391Z`

### Release Gateway

- Command: `$env:RUSTUP_TOOLCHAIN='stable'; cargo build --release --locked -p gateway`
- Result: passed
- Summary: release profile finished in 21.49 seconds with 9 `gateway-stt` warnings
- Build started: `2026-09-07T10:42:19.6689474Z`
- Build finished: `2026-09-07T10:42:41.2751764Z`
- Artifact: `target/release/promptforge-gateway.exe`
- Last modified: `2026-09-07T10:42:40.7268324Z`
- Size: 14,524,928 bytes
- SHA-256: `E3D4DD8694423F69DBE1A828BD2915C04E1E6963E2510867F6FC4A1E170C0FA4`

### Target-suffixed sidecar

- Command: `$triple='x86_64-pc-windows-msvc'; New-Item -ItemType Directory -Path 'crates\workshop\binaries' -Force | Out-Null; Copy-Item 'target\release\promptforge-gateway.exe' "crates\workshop\binaries\promptforge-gateway-$triple.exe" -Force`
- Artifact: `crates/workshop/binaries/promptforge-gateway-x86_64-pc-windows-msvc.exe`
- Last modified: `2026-09-07T10:42:40.7268324Z`
- Size: 14,524,928 bytes
- SHA-256: `E3D4DD8694423F69DBE1A828BD2915C04E1E6963E2510867F6FC4A1E170C0FA4`
- Verification: source and staged SHA-256 hashes matched at `2026-09-07T10:42:47.8474773Z`

### Packaging tool

- Command: `$env:RUSTUP_TOOLCHAIN='stable'; cargo install tauri-cli --locked`
- Result: passed
- Installed version: `tauri-cli 2.11.4`
- Detail: Cargo reported that the same version was already installed
- Verified: `2026-09-07T10:42:47.7978686Z`

### Fresh unsigned local NSIS package

- Exact successful PowerShell command: `cargo --% tauri build --bundles nsis --config {\"bundle\":{\"createUpdaterArtifacts\":false}}`
- Working directory: `crates/workshop`
- Result: passed
- Build started: `2026-09-07T10:42:55.4726299Z`
- Build finished: `2026-09-07T10:44:25.3444665Z`
- Workshop release profile finished in 59.98 seconds
- Installer: `target/release/bundle/nsis/PromptForge_0.2.0_x64-setup.exe`
- Installer created: `2026-09-07T10:43:58.3597272Z`
- Installer last modified: `2026-09-07T10:44:16.9471184Z`
- Installer size: 12,507,285 bytes
- Installer SHA-256: `DD2A21369B0834084F26D22ADAE92896431574506C607749F12FFC546ACB78D7`
- Previous installer SHA-256: `CD114A03A98F5E9F3354DC889998742E01EB3C221DB0CA61F0AA835DBDED885D`
- Freshness proof: the installer creation and modification timestamps follow the successful build start, and its hash differs from the previous installer
- Override scope: `bundle.createUpdaterArtifacts=false` was supplied only through the Tauri command line
- Protected release configuration: `crates/workshop/tauri.conf.json` and `.github/workflows/release-workshop.yml` have no diff
- Signing: not tested

The adjacent `PromptForge_0.2.0_x64-setup.exe.sig` remains stale from `2026-09-06T02:42:54.1030502Z` and is excluded from this build's evidence.

### Silent installation and installed identities

- Install result: passed
- Installer exit code: 0
- Install started: `2026-09-07T10:44:29.7572459Z`
- Install finished: `2026-09-07T10:44:33.1346052Z`
- Workshop path: `C:\Users\Vinnie\AppData\Local\PromptForge\promptforge-workshop.exe`
- Workshop version output: `promptforge-workshop 0.2.0`
- Workshop file version: `0.2.0`
- Workshop product version: `0.2.0`
- Workshop last modified: `2026-09-07T10:43:56Z`
- Workshop size: 24,881,664 bytes
- Workshop SHA-256: `3D95568DECE542DC0D56404FBFFB49567EAACED58E1A8CB11B5836572D33B8B6`
- Build-tree Workshop size: 24,881,664 bytes
- Build-tree Workshop SHA-256: `6779FC9020AC6CF48299E16F77EF1EBCE3533056185295F9DF13882E20FA618B`
- Workshop comparison: both identities are recorded without claiming equality because the Tauri log records NSIS bundle-information patching during packaging
- Gateway sibling path: `C:\Users\Vinnie\AppData\Local\PromptForge\promptforge-gateway.exe`
- Gateway sibling last modified: `2026-09-07T10:42:40Z`
- Gateway sibling size: 14,524,928 bytes
- Gateway sibling SHA-256: `E3D4DD8694423F69DBE1A828BD2915C04E1E6963E2510867F6FC4A1E170C0FA4`
- Gateway verification: installed, staged, and release SHA-256 hashes match
- Identity verification observed: `2026-09-07T10:44:43.0748367Z`

### Installed application launch

- Launched: `2026-09-07T10:44:51.0854084Z`
- Readiness-window observation: `2026-09-07T10:45:11.1809954Z`
- Workshop process ID: 95492
- Gateway process ID: 77192
- Both process paths resolve under `C:\Users\Vinnie\AppData\Local\PromptForge`
- Both processes remained running at `2026-09-07T10:45:38.3200361Z`
- No physical microphone, model-menu, or model-turn checklist item was observed during automated preparation

## Prior installed preparation after Steps 34 and 35

### Source and prior process boundary

- Current HEAD: `e7216d92c58f50d0c9b967bf4123e877b922cf47`
- Step 34 commit: `fb4e0bfe` (`Converge chat sessions with live catalogs`)
- Step 35 commit: `e7216d92` (`Partition live hypotheses into disjoint fields`)
- Installed Workshop or Gateway processes observed before rebuild: 0
- Installed Workshop or Gateway processes stopped: 0

### Release Gateway

- Command: `$env:RUSTUP_TOOLCHAIN='stable'; cargo build --release --locked -p gateway`
- Result: passed
- Summary: release profile finished in 26.57 seconds with 9 `gateway-stt` warnings
- Build started: `2026-09-07T09:39:44.2506384Z`
- Build finished: `2026-09-07T09:40:10.9436429Z`
- Artifact: `target/release/promptforge-gateway.exe`
- Last modified: `2026-09-07T09:40:10.4052397Z`
- Size: 14,477,824 bytes
- SHA-256: `D79453C2D91A6AF921C93C861624C8CCA4AC31497E1AF19AA38E458849E119DC`

### Target-suffixed sidecar

- Command: `$triple='x86_64-pc-windows-msvc'; New-Item -ItemType Directory -Path 'crates\workshop\binaries' -Force | Out-Null; Copy-Item 'target\release\promptforge-gateway.exe' "crates\workshop\binaries\promptforge-gateway-$triple.exe" -Force`
- Artifact: `crates/workshop/binaries/promptforge-gateway-x86_64-pc-windows-msvc.exe`
- Size: 14,477,824 bytes
- SHA-256: `D79453C2D91A6AF921C93C861624C8CCA4AC31497E1AF19AA38E458849E119DC`
- Verification: source and staged SHA-256 hashes matched at `2026-09-07T09:40:21.0845907Z`

### Packaging tool

- Command: `$env:RUSTUP_TOOLCHAIN='stable'; cargo install tauri-cli --locked`
- Result: passed
- Installed version: `tauri-cli 2.11.4`
- Detail: Cargo reported that the same version was already installed
- Verified: `2026-09-07T09:40:28.3091432Z`

### Fresh unsigned local NSIS package

- Exact successful PowerShell command: `cargo --% tauri build --bundles nsis --config {\"bundle\":{\"createUpdaterArtifacts\":false}}`
- Working directory: `crates/workshop`
- Result: passed
- Build started: `2026-09-07T09:40:48.847Z`
- Build finished: `2026-09-07T09:42:15.042Z`
- Workshop release profile finished in 59.28 seconds
- Installer: `target/release/bundle/nsis/PromptForge_0.2.0_x64-setup.exe`
- Installer created: `2026-09-07T09:41:51.6524557Z`
- Installer last modified: `2026-09-07T09:42:13.4020073Z`
- Installer size: 12,381,612 bytes
- Installer SHA-256: `CD114A03A98F5E9F3354DC889998742E01EB3C221DB0CA61F0AA835DBDED885D`
- Previous installer SHA-256: `CFBB5CBB539BE6B77FAB17BC9030E76CBA3D55B1DB21E84D5BD95CAF08E52606`
- Freshness proof: the installer creation and modification timestamps follow the successful build start, and its hash differs from the previous installer
- Override scope: `bundle.createUpdaterArtifacts=false` was supplied only through the Tauri command line
- Protected release configuration: `crates/workshop/tauri.conf.json` and `.github/workflows/release-workshop.yml` have no diff
- Signing: not tested

The adjacent `PromptForge_0.2.0_x64-setup.exe.sig` remains stale from `2026-09-06T02:42:54.1030502Z` and is excluded from this build's evidence.

### Silent installation and installed identities

- Install result: passed
- Installer exit code: 0
- Install started: `2026-09-07T09:42:28.9881969Z`
- Install finished: `2026-09-07T09:42:32.3954431Z`
- Workshop path: `C:\Users\Vinnie\AppData\Local\PromptForge\promptforge-workshop.exe`
- Workshop file version: `0.2.0`
- Workshop product version: `0.2.0`
- Workshop last modified: `2026-09-07T09:41:50Z`
- Workshop size: 24,290,816 bytes
- Workshop SHA-256: `0BCD250129F2D7B1BE5218326C7FEF8FC93238ACE69CC73AFFF6648FB0F6FE74`
- Build-tree Workshop size: 24,290,816 bytes
- Build-tree Workshop SHA-256: `22897B508E501402B4FF17A207917AD3BF30C573B374E2F4DA671BA8DA160EE8`
- Workshop comparison: both identities are recorded without claiming equality because the Tauri log records NSIS bundle-information patching during packaging
- Gateway sibling path: `C:\Users\Vinnie\AppData\Local\PromptForge\promptforge-gateway.exe`
- Gateway sibling last modified: `2026-09-07T09:40:10Z`
- Gateway sibling size: 14,477,824 bytes
- Gateway sibling SHA-256: `D79453C2D91A6AF921C93C861624C8CCA4AC31497E1AF19AA38E458849E119DC`
- Gateway verification: installed, staged, and release SHA-256 hashes match
- Identity verification observed: `2026-09-07T09:42:42.8152106Z`

### Installed application launch

- Launched: `2026-09-07T09:42:51.6396972Z`
- Readiness-window observation: `2026-09-07T09:43:11.7819091Z`
- Workshop process ID: 79208
- Gateway process ID: 60656
- Both process paths resolve under `C:\Users\Vinnie\AppData\Local\PromptForge`
- Both processes remained running at `2026-09-07T09:43:59.6143596Z`
- No physical microphone, model-menu, or model-turn checklist item was observed during automated preparation

## Prior installed preparation before Steps 34 and 35

### Source and release Gateway

- Current HEAD: `aeec7b48fad441f42e5b66ec09274f50455180eb`
- Command: `$env:RUSTUP_TOOLCHAIN='stable'; cargo build --release --locked -p gateway`
- Result: passed
- Summary: release profile finished in 27.14 seconds with 9 `gateway-stt` warnings
- Build started: `2026-09-07T07:22:02.4562698Z`
- Build finished: `2026-09-07T07:22:29.7203501Z`
- Artifact: `target/release/promptforge-gateway.exe`
- Last modified: `2026-09-07T07:22:29.1910374Z`
- Size: 14,459,904 bytes
- SHA-256: `8A1D73EDD4BE102482B5B7DAF253B09F6D90C51EA0EA13EE439ECAA4E9DF5DEB`

### Target-suffixed sidecar

- Command: `$triple='x86_64-pc-windows-msvc'; New-Item -ItemType Directory -Path 'crates\workshop\binaries' -Force | Out-Null; Copy-Item 'target\release\promptforge-gateway.exe' "crates\workshop\binaries\promptforge-gateway-$triple.exe" -Force`
- Artifact: `crates/workshop/binaries/promptforge-gateway-x86_64-pc-windows-msvc.exe`
- Size: 14,459,904 bytes
- SHA-256: `8A1D73EDD4BE102482B5B7DAF253B09F6D90C51EA0EA13EE439ECAA4E9DF5DEB`
- Verification: source and staged SHA-256 hashes matched at `2026-09-07T07:22:35.0127460Z`

### Packaging tool

- Command: `$env:RUSTUP_TOOLCHAIN='stable'; cargo install tauri-cli --locked`
- Result: passed
- Installed version: `tauri-cli 2.11.4`
- Detail: Cargo reported that the same version was already installed
- Verified: `2026-09-07T07:25:14.1899756Z`

### Fresh unsigned local NSIS package

- Authorized command: `cargo tauri build --bundles nsis --config '{"bundle":{"createUpdaterArtifacts":false}}'`
- Working directory: `crates/workshop`
- Result: passed
- Successful build started: `2026-09-07T07:22:58.5565020Z`
- Successful build finished: `2026-09-07T07:24:11.0138047Z`
- Installer: `target/release/bundle/nsis/PromptForge_0.2.0_x64-setup.exe`
- Installer created: `2026-09-07T07:23:52.9874435Z`
- Installer last modified: `2026-09-07T07:24:10.9102655Z`
- Installer size: 12,374,918 bytes
- Installer SHA-256: `CFBB5CBB539BE6B77FAB17BC9030E76CBA3D55B1DB21E84D5BD95CAF08E52606`
- Previous installer SHA-256: `750DBEF0F96FC9AE4364942856E558E2D951731CEE8BDC607397A36A457599AB`
- Freshness proof: the installer creation and modification timestamps follow the successful build start, and its hash differs from the previous installer
- Override scope: `bundle.createUpdaterArtifacts=false` was supplied only through the Tauri command line
- Protected release configuration: `crates/workshop/tauri.conf.json` and `.github/workflows/release-workshop.yml` have no diff
- Signing: not tested

The adjacent `PromptForge_0.2.0_x64-setup.exe.sig` remains stale from `2026-09-06T02:42:54.1030502Z` and is excluded from this build's evidence.

### Silent installation and installed identities

- Install result: passed
- Installer exit code: 0
- Install started: `2026-09-07T07:24:18.0065320Z`
- Install finished: `2026-09-07T07:24:21.3903424Z`
- Workshop path: `C:\Users\Vinnie\AppData\Local\PromptForge\promptforge-workshop.exe`
- Workshop version output: `promptforge-workshop 0.2.0`
- Workshop last modified: `2026-09-07T07:23:50Z`
- Workshop size: 24,245,248 bytes
- Workshop SHA-256: `42E7D7500425F91AE576F4E1CAE5E11DE0B606EF1836E8EC1A2EEABD059B7A73`
- Build-tree Workshop SHA-256: `A890D75817337D68A1E8660F8B11F11E7216A9D71C05625A712E9193E8E35CBD`
- Workshop comparison: both identities are recorded without claiming equality because the Tauri log records NSIS bundle-information patching during packaging
- Gateway sibling path: `C:\Users\Vinnie\AppData\Local\PromptForge\promptforge-gateway.exe`
- Gateway sibling last modified: `2026-09-07T07:22:28Z`
- Gateway sibling size: 14,459,904 bytes
- Gateway sibling SHA-256: `8A1D73EDD4BE102482B5B7DAF253B09F6D90C51EA0EA13EE439ECAA4E9DF5DEB`
- Gateway verification: installed, staged, and release SHA-256 hashes match
- Identity verification observed: `2026-09-07T07:24:36.4332156Z`

### Installed application launch

- Launched through Windows Explorer for operator handoff: `2026-09-07T07:26:12.8249425Z`
- Readiness-window observation: `2026-09-07T07:26:25.7625183Z`
- Separate post-handoff observation: `2026-09-07T07:26:37.3855897Z`
- Workshop process ID: 83436
- Gateway process ID: 98380
- Both process paths resolve under `C:\Users\Vinnie\AppData\Local\PromptForge`
- No physical microphone or model-turn checklist item was observed during automated preparation

## Completed automated prerequisites

### Gateway Realtime STT

- Command: `cargo test -p gateway --test it realtime_stt`
- Result: passed
- Summary: 9 passed, 0 failed, 0 ignored, 78 filtered out
- Finished: `2026-09-07T01:55:48.700Z`

### Workshop Realtime relay

- Command: `cargo test -p workshop-server --test it realtime_relay`
- Result: passed
- Summary: 7 passed, 0 failed, 0 ignored, 30 filtered out
- Finished: `2026-09-07T01:56:27.864Z`

### Workshop UI

- Working directory: `crates/workshop-server/ui`
- Command: `npm test`
- Result: passed
- Summary: 71 passed, 0 failed, 0 cancelled, 0 skipped
- Finished: `2026-09-07T01:56:05.209Z`

## Completed release preparation

### Release Gateway

- Command: `$env:RUSTUP_TOOLCHAIN='stable'; cargo build --release --locked -p gateway`
- Result: passed
- Summary: release profile finished in 1 minute 52 seconds with 9 `gateway-stt` warnings
- Finished: `2026-09-07T01:58:30.071Z`
- Artifact: `target/release/promptforge-gateway.exe`
- Size: 14,457,344 bytes
- SHA-256: `7211CA8E7D71533274EADD6264A77781D1F893B746B76EB8A560280770A7120F`

### Target-suffixed sidecar

- Command: `$triple='x86_64-pc-windows-msvc'; New-Item -ItemType Directory -Path 'crates\workshop\binaries' -Force | Out-Null; Copy-Item 'target\release\promptforge-gateway.exe' "crates\workshop\binaries\promptforge-gateway-$triple.exe" -Force`
- Result: passed
- Artifact: `crates/workshop/binaries/promptforge-gateway-x86_64-pc-windows-msvc.exe`
- Size: 14,457,344 bytes
- SHA-256: `7211CA8E7D71533274EADD6264A77781D1F893B746B76EB8A560280770A7120F`
- Verification: source and staged SHA-256 hashes match

### Packaging tool

- Command: `$env:RUSTUP_TOOLCHAIN='stable'; cargo install tauri-cli --locked`
- Result: passed
- Installed version: `tauri-cli 2.11.4`
- Detail: Cargo reported that the same version was already installed
- Finished: `2026-09-07T01:58:42.463Z`

## Unsigned local NSIS build

- Authorized command: `cargo tauri build --bundles nsis --config '{"bundle":{"createUpdaterArtifacts":false}}'`
- Working directory: `crates/workshop`
- Result: passed
- Build started: `2026-09-07T02:17:35.0140268Z`
- Build finished: `2026-09-07T02:19:29.0773256Z`
- Installer: `target/release/bundle/nsis/PromptForge_0.2.0_x64-setup.exe`
- Installer created: `2026-09-07T02:19:10Z`
- Installer last modified: `2026-09-07T02:19:28Z`
- Installer size: 12,360,736 bytes
- Installer SHA-256: `D045D0C4F57702AB42C93BBE1CEEC810A900C1952DAD1DE456011854899E5520`
- Freshness proof: installer creation and modification timestamps are later than the successful attempt's start timestamp
- Override scope: `bundle.createUpdaterArtifacts=false` was supplied only on the Tauri command line
- Protected files: `crates/workshop/tauri.conf.json` and `.github/workflows/release-workshop.yml` have no diff
- Signing: not tested

The successful unsigned attempt did not create a current `.sig` file. The adjacent `PromptForge_0.2.0_x64-setup.exe.sig` is stale from `2026-09-06T02:42:54Z` and is excluded from this run's evidence.

## Silent installation

- Command: `Start-Process $setup.FullName -ArgumentList '/S' -Wait -PassThru`
- Result: passed
- Installer exit code: 0
- Started: `2026-09-07T02:19:41.9986519Z`
- Finished: `2026-09-07T02:19:45.3941424Z`

## Installed sibling verification

### Workshop

- Path: `C:\Users\Vinnie\AppData\Local\PromptForge\promptforge-workshop.exe`
- Size: 24,210,432 bytes
- SHA-256: `B0B8B3A7D732CBF3B1982AF1CB588DB20F3A0BECD5ED5A508F65EF6E26459B28`
- Version output: `promptforge-workshop 0.2.0`
- Sibling Gateway present: yes

The Workshop build-tree executable has the same size but SHA-256 `169639C71AD94018FCA0F37E7977B607508EBDE59FE89B5DB4EB34A85366361C`. This is not treated as an applicable byte-for-byte comparison because the Tauri log records patching the Workshop executable with NSIS bundle information during packaging. Both hashes are recorded rather than claiming equality.

### Gateway

- Path: `C:\Users\Vinnie\AppData\Local\PromptForge\promptforge-gateway.exe`
- Size: 14,457,344 bytes
- SHA-256: `7211CA8E7D71533274EADD6264A77781D1F893B746B76EB8A560280770A7120F`
- Staged sidecar SHA-256: `7211CA8E7D71533274EADD6264A77781D1F893B746B76EB8A560280770A7120F`
- Release Gateway SHA-256: `7211CA8E7D71533274EADD6264A77781D1F893B746B76EB8A560280770A7120F`
- Verification: installed, staged, and release Gateway hashes match

## Installed application launch

- Launched: `2026-09-07T02:20:39.1019801Z`
- Workshop process ID: 78444
- Gateway sibling process observed: yes
- Gateway process ID: 101204
- Both installed processes remained running at the automated handoff

## Second installed attempt after no-model-turn fix

### Source and process boundary

- Current HEAD: `49441166f580a3d6339532a3c1fd3c1205e484cd`
- Installed processes observed before rebuild: 0
- Installed processes stopped: 0
- Installed processes remaining before rebuild: 0
- Process boundary observed: `2026-09-07T04:26:54.2378279Z`

### Release Gateway and staged sidecar

- Command: `$env:RUSTUP_TOOLCHAIN='stable'; cargo build --release --locked -p gateway`
- Result: passed
- Summary: release profile finished in 49.18 seconds with 9 `gateway-stt` warnings
- Finished: `2026-09-07T04:27:44.975Z`
- Release Gateway: `target/release/promptforge-gateway.exe`
- Release Gateway last modified: `2026-09-07T04:27:42.7135449Z`
- Release Gateway size: 14,457,344 bytes
- Release Gateway SHA-256: `7BE1C818B196A1C889ADE75D8AA09847404808C6530FFC7662DBBC26E10BAD18`
- Staged sidecar: `crates/workshop/binaries/promptforge-gateway-x86_64-pc-windows-msvc.exe`
- Staged sidecar size: 14,457,344 bytes
- Staged sidecar SHA-256: `7BE1C818B196A1C889ADE75D8AA09847404808C6530FFC7662DBBC26E10BAD18`
- Staging verified: `2026-09-07T04:27:55.0750924Z`
- Verification: current release and staged Gateway hashes match

### Fresh unsigned local NSIS package

- Authorized command: `cargo tauri build --bundles nsis --config '{"bundle":{"createUpdaterArtifacts":false}}'`
- Working directory: `crates/workshop`
- Result: passed
- Previous installer last modified: `2026-09-07T02:19:28.9716724Z`
- Previous installer SHA-256: `D045D0C4F57702AB42C93BBE1CEEC810A900C1952DAD1DE456011854899E5520`
- Build started: `2026-09-07T04:28:04.7578431Z`
- Build finished: `2026-09-07T04:29:35.1883254Z`
- Current installer: `target/release/bundle/nsis/PromptForge_0.2.0_x64-setup.exe`
- Current installer created: `2026-09-07T04:29:17Z`
- Current installer last modified: `2026-09-07T04:29:35Z`
- Current installer size: 12,359,149 bytes
- Current installer SHA-256: `750DBEF0F96FC9AE4364942856E558E2D951731CEE8BDC607397A36A457599AB`
- Freshness proof: the current installer creation and modification timestamps follow this attempt's start, and its hash differs from the previous installer
- Override scope: `bundle.createUpdaterArtifacts=false` was supplied only on the Tauri command line
- Protected release configuration: unchanged
- Signing: not tested

### Second silent installation

- Result: passed
- Installer exit code: 0
- Started: `2026-09-07T04:29:47.7712059Z`
- Finished: `2026-09-07T04:29:51.1584956Z`

### Second installed identities

- Workshop path: `C:\Users\Vinnie\AppData\Local\PromptForge\promptforge-workshop.exe`
- Workshop version output: `promptforge-workshop 0.2.0`
- Workshop file version: `0.2.0`
- Workshop last modified: `2026-09-07T04:29:14Z`
- Workshop size: 24,211,456 bytes
- Workshop SHA-256: `AD2A99A912F016C7B11B37DEAA29DF6A04AB2292BD8C56E1C3D6065585D29B35`
- Build-tree Workshop SHA-256: `A0638A0B723997026CFE92DC056597E13D83DBB032B18B8D7453DC2AE513246F`
- Workshop comparison: both identities are recorded without claiming equality because the Tauri log records NSIS bundle-information patching during packaging
- Gateway sibling path: `C:\Users\Vinnie\AppData\Local\PromptForge\promptforge-gateway.exe`
- Gateway sibling last modified: `2026-09-07T04:27:42Z`
- Gateway sibling size: 14,457,344 bytes
- Gateway sibling SHA-256: `7BE1C818B196A1C889ADE75D8AA09847404808C6530FFC7662DBBC26E10BAD18`
- Gateway verification: installed, staged, and release SHA-256 hashes match
- Identity verification observed: `2026-09-07T04:30:01.8473944Z`

### Second installed launch

- Launched: `2026-09-07T04:30:08.4151156Z`
- Workshop process ID: 48564
- Gateway process ID: 59724
- Both process paths resolve under `C:\Users\Vinnie\AppData\Local\PromptForge`
- Both processes remained running at `2026-09-07T04:30:16.5398864Z`

## Operator observations - later build-tree launch

- Observed: approximately `2026-09-07T06:33Z` through `2026-09-07T06:36Z`
- Acceptance applicability: none; process inspection showed both Workshop and Gateway running from `C:\Users\Vinnie\cursor\promptforge\target\release`, not the installed `AppData\Local\PromptForge` paths
- Gateway readiness: serving at `06:33:11Z`, speech ready with GPU and profile switched by `06:33:13Z`, `claude-opus-4-6` advertised, chat endpoint ready
- Workshop model catalog: failed; the picker exposed no model even though Gateway advertised `claude-opus-4-6`
- Realtime connection: failed latency; microphone readiness took approximately 20 to 30 seconds
- Live hypotheses: failed; no text evolved while recording
- Completion: functional; correct text appeared only after stop
- Status lifecycle: failed; the progress bar remained visible after profile completion and the normal LEDs did not return
- Diagnosis: Workshop refreshed model state before Gateway profile publication and did not retry while health stayed reachable; precommit hypothesis IDs were not bound to the active take; the imported Gateway progress operation remained attached to the never-ending SSE stream after its root finished

## Prior failed observations - first installed attempt

### Dictation

- Observed: approximately `2026-09-07T03:55Z` through `2026-09-07T03:57Z`
- Result: partial success, latency failure
- Evidence: the installed Workshop first displayed `Dictation is connecting. Try again in a moment.`, then eventually inserted `Tell me a story, is it gonna work? I don't think it's gonna work.`
- Connection delay: 5 to 15 seconds
- Stop-to-final delay: 5 to 15 seconds
- Verdict: the installed speech path works functionally, but both observed delays exceed the two-second acceptance budget

### Model turn after dictation

- Observed: approximately `2026-09-07T03:57Z`
- Result: failed
- Evidence: the model picker still displayed `Select model`; submission persisted the user message and a tool result containing the dictated text, but no assistant message followed
- Session log: `dd27eef4544a74d2b12e9f1a25251000`
- Gateway state during diagnosis: running, profile `default`, `claude-opus-4-6` advertised, chat endpoint ready, speech ready with GPU, no active or pending command
- Network state during diagnosis: Workshop retained local Gateway connections, while Gateway held no outbound provider connection
- Verdict: no Anthropic request was reached; the local no-model binding error returned through Lua `pcall` without an operator-visible response

## Operator observation checklist retained for audit context

This checklist records the originally requested observation detail. Unchecked items were not individually recorded and are not retroactively claimed as measured; the operator's later authoritative verdict for the identified installed build is the acceptance decision.

- [ ] Confirm both chat model menus, the inline dropdown and the top-level `Model` menu, show only chat-capable models and do not list speech-only models.
- [ ] Select `claude-opus-4-6`, submit typed input, and confirm the selected Claude model completes the turn with an assistant response.
- [ ] Confirm live speech revisions replace rather than duplicate provisional text, with exact spacing preserved.
- [ ] Confirm completion commits the final transcript exactly once.
- [ ] Start a second take and confirm it is independent of the first.
- [ ] Clear the transcript and confirm the visible and retained take state clears.
- [ ] Cancel an active take and confirm no later hypothesis or completion is applied.
- [ ] Deny microphone permission or select an unavailable device, confirm a recoverable error, restore access, and confirm a new take works.
- [ ] Measure connection delay from microphone activation to ready capture.
- [ ] Measure stop-to-final delay from stop action to committed final transcript.
- [ ] Record the installed Workshop path, sibling Gateway path, installer path, sizes, SHA-256 hashes, and UTC timestamps.

Automated preparation did not perform the checklist. The operator later observed the latest installed build and accepted it as recorded below, without supplying measurements or item-by-item results beyond those stated.

## Operator acceptance - post-Step 37 installed build

- Observed: approximately `2026-09-07T12:15Z`
- Build under test: installed unsigned package built from `2d1ecca8`
- Short-utterance Stop regression: passed; repeated utterances with the last word spoken immediately before Stop retained the correct final word
- Live transcription: passed; operator reported the repaired behavior works correctly
- Overall operator verdict: `Works correctly. Accepted.`
- Signing: not tested; release signing remains a release-CI gate

## Operator observations - post-Step 36 installed attempt

- Observed: approximately `2026-09-07T10:59Z`
- Live hypotheses: substantially improved; the prior repeated-phrase accumulation was not observed
- Stop finalization: failed intermittently; a correct word appeared in the latest live hypothesis, then pressing Stop removed that word from the authoritative completion
- Verdict: cadence and whole-window rebasing improved the live path, but Step 37 remains failed because completion can discard recognized audio-backed tail text

## Operator observations - post-Steps 34 and 35 installed attempt

- Observed: approximately `2026-09-07T09:47Z`
- Chat model menus: passed; speech-only models no longer appeared
- Typed model turn: passed; selected chat model responded
- Live hypotheses: failed; provisional text still accumulated repeated phrases while recording instead of presenting one evolving replacement
- Completion: prior behavior indicates Stop replaces provisional text with the clean authoritative final, but the full completion checklist was not repeated in this observation
- Verdict: Step 34 repairs passed installed observation; Step 35 did not repair the real native interim sequence, so Step 36 remains failed

## Prior failed observations - pre-Steps 34 and 35 installed attempt

- Observed: approximately `2026-09-07T08:45Z`
- Model catalog: `claude-opus-4-6` was visible and selected
- Chat model menus: failed filtering; both the inline dropdown and top-level `Model` menu listed `whisper-base-en`, `whisper-small-en`, and `realtime-transcribe`, which are speech models and must not be selectable for chat
- Typed model turn: failed; submitting `test 1 2 3` persisted the user input and tool result, then displayed `Error: Model turn failed in agent 'chat'`
- Model-turn diagnosis: Workshop launched the built-in chat session before Gateway published its profile models, freezing an empty session model catalog; later catalog convergence updated the picker but not that running session, so binding failed locally before any Gateway completion request
- Live hypotheses: failed replacement behavior; revisions appeared while recording but accumulated repeatedly in the editor
- Completion: functional replacement; pressing Stop removed the duplicated provisional text and left the correct final transcript
- Verdict: Step 34 remains failed; model-session catalog convergence and live ProseMirror range replacement require repair before acceptance can be repeated
