"""Render the sample exams into "phone photos" (JPEG) and a PDF for the real ingest test.

usage: python -I render.py <out_dir> [chrome.exe]
de1.html -> de1_p1.jpg, de1_p2.jpg, ... (slightly rotated, tinted, blurred, like photos)
phieu.html -> phieu.jpg ; de2.html -> de2.pdf (A4)
Needs playwright + Pillow. KaTeX comes from shared/board/vendor/katex.
"""
import pathlib
import random
import re
import sys

from PIL import Image, ImageEnhance, ImageFilter
from playwright.sync_api import sync_playwright

HERE = pathlib.Path(__file__).resolve().parent
KATEX = (HERE / "../../../../shared/board/vendor/katex").resolve()
OUT = pathlib.Path(sys.argv[1]).resolve()
CHROME = sys.argv[2] if len(sys.argv) > 2 else None
OUT.mkdir(parents=True, exist_ok=True)
random.seed(7)

TEX = re.compile(r"\$(.+?)\$", re.S)


def prepare(name):
    html = (HERE / name).read_text(encoding="utf-8").replace("KATEX/", KATEX.as_uri() + "/")
    html = TEX.sub(lambda m: '<span class="tx" data-t="%s"></span>' % m.group(1).replace('"', "&quot;"), html)
    html = html.replace("</body>", '<script src="%s/katex.min.js"></script><script>'
                        'document.querySelectorAll(".tx").forEach(e=>katex.render(e.dataset.t,e,{throwOnError:false}));'
                        '</script></body>' % KATEX.as_uri())
    p = OUT / ("_" + name)
    p.write_text(html, encoding="utf-8")
    return p


def photo(img, path):
    img = img.convert("RGB")
    img = ImageEnhance.Color(img).enhance(0.9)
    tint = Image.new("RGB", img.size, (250, 240, 215))
    img = Image.blend(img, tint, 0.18)
    img = img.rotate(random.uniform(-1.8, 1.8), resample=Image.BICUBIC, expand=True, fillcolor=(120, 110, 95))
    img = img.filter(ImageFilter.GaussianBlur(0.6))
    img = ImageEnhance.Brightness(img).enhance(random.uniform(0.92, 1.02))
    w, h = img.size
    scale = min(1.0, 2000 / max(w, h))
    img = img.resize((int(w * scale), int(h * scale)))
    img.save(path, "JPEG", quality=80)


with sync_playwright() as p:
    b = p.chromium.launch(executable_path=CHROME) if CHROME else p.chromium.launch()
    pg = b.new_page(viewport={"width": 820, "height": 1000}, device_scale_factor=2)
    for name, parts in (("de1.html", 3), ("phieu.html", 1)):
        pg.goto(prepare(name).as_uri())
        pg.wait_for_timeout(800)
        full = OUT / ("_" + name + ".png")
        pg.screenshot(path=str(full), full_page=True)
        img = Image.open(full)
        w, h = img.size
        step = h // parts
        for i in range(parts):
            top = max(0, i * step - 40)
            bottom = min(h, (i + 1) * step + 40)
            out = OUT / (name.replace(".html", "") + ("_p%d" % (i + 1) if parts > 1 else "") + ".jpg")
            photo(img.crop((0, top, w, bottom)), out)
            print(out)
    pg.goto(prepare("de2.html").as_uri())
    pg.wait_for_timeout(800)
    pg.pdf(path=str(OUT / "de2.pdf"), format="A4", print_background=True)
    print(OUT / "de2.pdf")
    b.close()
