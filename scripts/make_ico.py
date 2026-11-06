"""Build assets/marquee.ico: simplified glyph at 16/24 px, full icon above."""
import sys
from PIL import Image

win = Image.open("assets/icon-win-1024.png").convert("RGBA")
small = Image.open("assets/icon-small-256.png").convert("RGBA")
frames = [(small if s <= 24 else win).resize((s, s), Image.LANCZOS) for s in (16, 20, 24, 32, 40, 48, 64, 128, 256)]
frames[-1].save("assets/marquee.ico", format="ICO", sizes=[f.size for f in frames], append_images=frames[:-1])
print("wrote assets/marquee.ico")
