# -*- coding: utf-8 -*-
"""生成 assets/app.ico：蓝底圆角方块 + 白色闪电，多尺寸。"""
from __future__ import annotations

from pathlib import Path

from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "assets"
OUT.mkdir(exist_ok=True)

SIZE = 256
img = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
draw = ImageDraw.Draw(img)

# 圆角底：纵向渐变蓝
top, bottom = (37, 99, 235), (29, 78, 216)
grad = Image.new("RGBA", (SIZE, SIZE))
gdraw = ImageDraw.Draw(grad)
for y in range(SIZE):
    t = y / (SIZE - 1)
    color = tuple(round(a + (b - a) * t) for a, b in zip(top, bottom)) + (255,)
    gdraw.line([(0, y), (SIZE, y)], fill=color)
mask = Image.new("L", (SIZE, SIZE), 0)
ImageDraw.Draw(mask).rounded_rectangle([8, 8, SIZE - 8, SIZE - 8], radius=56, fill=255)
img.paste(grad, (0, 0), mask)

# 白色闪电
bolt = [(150, 34), (84, 142), (124, 142), (102, 222), (176, 108), (132, 108), (158, 34)]
draw.polygon(bolt, fill=(255, 255, 255, 255))

img.save(OUT / "app.ico", sizes=[(256, 256), (128, 128), (64, 64), (48, 48), (32, 32), (16, 16)])
img.resize((128, 128), Image.LANCZOS).save(OUT / "app.png")
print("icon written:", OUT / "app.ico")
