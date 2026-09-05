# Icons Directory - Agent Rules

## Derived Copies

`crates/gateway-config-ui/ui/icons/promptforge-icon.png` and `crates/workshop-server/ui/icons/promptforge-icon.png` are copies of `128x128.png`, and the `promptforge-icon@2x.png` beside each is a copy of `128x128@2x.png`. When this set is regenerated, refresh those four files in the same change. `promptforge-gateway.exe` embeds `icon.ico` through the gateway crate's `build.rs`.

## Do Not Touch

The following files are hand-crafted installer chrome assets. Do not regenerate, resize, overwrite, or modify them:

- `installer-header.png` - NSIS header source (PNG master)
- `installer-header.bmp` - NSIS header (converted from PNG, do not edit directly)
- `installer-sidebar.png` - NSIS sidebar source (PNG master)
- `installer-sidebar.bmp` - NSIS sidebar (converted from PNG, do not edit directly)
- `dmg-background.png` - macOS DMG background

If the PNGs are updated, regenerate the BMPs with:

```python
from PIL import Image
img = Image.open("installer-header.png").convert("RGB")
img.save("installer-header.bmp", "BMP")
img = Image.open("installer-sidebar.png").convert("RGB")
img.save("installer-sidebar.bmp", "BMP")
```
