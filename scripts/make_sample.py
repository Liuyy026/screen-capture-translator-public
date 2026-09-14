"""Generate an original, non-explicit fixture for horizontal/vertical OCR QA."""
from pathlib import Path
from PIL import Image, ImageDraw, ImageFont

root = Path(__file__).resolve().parents[1]
dest = root / 'samples'
dest.mkdir(exist_ok=True)
font = ImageFont.truetype('/Library/Fonts/Arial Unicode.ttf', 36)
small = ImageFont.truetype('/Library/Fonts/Arial Unicode.ttf', 22)
im = Image.new('RGB', (1000, 1400), '#f9f7f1')
d = ImageDraw.Draw(im)
d.text((55, 25), 'KOMA / OCR TEST · 01', font=small, fill='#666666')
for rect in [(45, 85, 955, 665), (45, 695, 955, 1350)]:
    d.rectangle(rect, fill='white', outline='black', width=5)
# Simple scenery deliberately separate from the speech bubbles.
d.line([(70, 595), (350, 450), (610, 590), (930, 400)], fill='#999999', width=3)
d.line([(70, 625), (930, 625)], fill='black', width=3)
d.ellipse((695, 130, 900, 505), fill='white', outline='black', width=3)
d.ellipse((130, 150, 490, 340), fill='white', outline='black', width=3)
for i, char in enumerate('今日はいい天気だね'):
    d.text((790, 155 + i * 29), char, font=font, fill='black')
d.text((195, 220), 'そうだね。', font=font, fill='black')
d.ellipse((630, 755, 900, 1200), fill='white', outline='black', width=3)
for i, char in enumerate('一緒に散歩しよう'):
    d.text((755, 805 + i * 34), char, font=font, fill='black')
d.ellipse((120, 860, 490, 1070), fill='white', outline='black', width=3)
d.text((175, 940), 'ちょっと待って！', font=font, fill='black')
im.save(dest / '01.webp', lossless=True)
# Numbering fixture also exercises natural filename ordering.
im2 = Image.new('RGB', (1000, 1400), 'white')
d2 = ImageDraw.Draw(im2)
d2.rectangle((45, 85, 955, 1300), outline='black', width=5)
d2.ellipse((160, 240, 850, 590), outline='black', width=3)
d2.text((240, 360), '準備できたよ。行こう！', font=font, fill='black')
im2.save(dest / '02.webp', lossless=True)
print(dest)
