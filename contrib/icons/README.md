# PWSW Icons

## For Packagers

Install the PNG sizes and the SVG (as the scalable icon), as `contrib/PKGBUILD` does:

```bash
for size in 16 24 32 48 64 128 256 512; do
    install -Dm644 "pwsw-${size}.png" "$pkgdir/usr/share/icons/hicolor/${size}x${size}/apps/pwsw.png"
done
install -Dm644 pwsw.svg "$pkgdir/usr/share/icons/hicolor/scalable/apps/pwsw.svg"
```

## Regenerating PNGs

The SVG is kept as the source file for future edits. To regenerate the PNGs:

**Requirements:**
- Inkscape 1.4+

**Command:**
```bash
for size in 16 24 32 48 64 128 256 512; do
    inkscape contrib/icons/pwsw.svg --export-filename="contrib/icons/pwsw-${size}.png" --export-width=$size --export-height=$size
done
```
