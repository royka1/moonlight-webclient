# Icons

`icon.svg` is the only icon currently committed. The PWA manifest references
the following PNG sizes; generate them from the SVG before shipping:

- `icon-192.png` (192x192)
- `icon-512.png` (512x512)
- `icon-maskable-512.png` (512x512, with safe-area padding for maskable icon)

Quick generation:

```sh
# Using rsvg-convert (librsvg)
rsvg-convert -w 192 -h 192 icon.svg -o icon-192.png
rsvg-convert -w 512 -h 512 icon.svg -o icon-512.png
rsvg-convert -w 512 -h 512 icon.svg -o icon-maskable-512.png

# Or using ImageMagick
magick icon.svg -resize 192x192 icon-192.png
```
