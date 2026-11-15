"""Harder benchmark pages with known ground truth, plus the ground truth of the 4 fixture photos.

usage: python -I -X utf8 hard_pages.py <pages_dir> [chrome.exe]
Writes <pages_dir>/hard_*.jpg (phone-photo look, like render.py) and ground_truth.json next to this
script. The fixture photos de1_p1..p3.jpg and phieu.jpg come from render.py; their ground truth below was
written by reading those rendered photos (de1 is cut into 3 photos: Câu 6 starts at the very bottom of p1,
Bài 1's formula starts at the bottom of p2).

Ground truth per question: key (section.n), kind, `math` = LaTeX snippets that must be transcribed
(compared after normalisation, see examples/ingest_bench.rs), `vi` = Vietnamese phrases that must appear
with their diacritics, `chem` = compare formulas with the chemistry normalisation.
"""
import json
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
random.seed(11)


def Q(sec, n, kind, text, choices=None, math=(), vi=(), chem=False, figure=False, label=None, sub=None, statements=None):
    return dict(sec=sec, n=n, kind=kind, text=text, choices=choices, math=list(math), vi=list(vi), chem=chem,
                figure=figure, label=label, sub=sub, statements=statements)


# ------------------------------------------------------------------ hard_math: Toán 12, 4 parts, numbering restarts
HARD_MATH = dict(
    id="hard_math", file="hard_math.jpg", kind="exam", full=True,
    head=("SỞ GD&ĐT HÀ NỘI<br>TRƯỜNG THPT CHU VĂN AN",
          "ĐỀ KIỂM TRA CUỐI HỌC KÌ I<br>MÔN: TOÁN 12", "<i>Thời gian làm bài: 90 phút</i>"),
    title="ĐỀ KIỂM TRA CUỐI HỌC KÌ I – MÔN TOÁN 12",
    parts=[
        ("I", "PHẦN I. Câu trắc nghiệm nhiều phương án lựa chọn", [
            Q("I", "1", "mcq", r"Tập xác định của hàm số $y=\log_{2}(x^2-4x+3)$ là",
              [r"$(-\infty;1)\cup(3;+\infty)$", r"$[1;3]$", r"$(1;3)$", r"$\mathbb{R}\setminus\{1;3\}$"],
              math=[r"\log_{2}(x^2-4x+3)", r"(-\infty;1)\cup(3;+\infty)", r"\mathbb{R}\setminus\{1;3\}"], vi=["Tập xác định của hàm số"]),
            Q("I", "2", "mcq", r"Tích phân $\displaystyle\int_{0}^{1}(3x^2-2x+1)\,dx$ bằng", [r"$1$", r"$2$", r"$\dfrac{1}{2}$", r"$0$"],
              math=[r"\int_{0}^{1}(3x^2-2x+1)dx"], vi=["Tích phân"]),
            Q("I", "3", "mcq", r"Giới hạn $\lim\limits_{x\to 2}\dfrac{\sqrt{x+2}-2}{x-2}$ bằng",
              [r"$\dfrac{1}{4}$", r"$\dfrac{1}{2}$", r"$0$", r"$+\infty$"],
              math=[r"\lim_{x\to2}\frac{\sqrt{x+2}-2}{x-2}", r"+\infty"], vi=["Giới hạn"]),
            Q("I", "4", "mcq", r"Đạo hàm của hàm số $y=e^{2x}\sin x$ là",
              [r"$y'=e^{2x}(2\sin x+\cos x)$", r"$y'=2e^{2x}\cos x$", r"$y'=e^{2x}(\sin x+2\cos x)$", r"$y'=e^{2x}\cos x$"],
              math=[r"y=e^{2x}\sin x", r"e^{2x}(2\sin x+\cos x)", r"e^{2x}(\sin x+2\cos x)"], vi=["Đạo hàm của hàm số"]),
            Q("I", "5", "mcq", r"Nghiệm của phương trình $3^{2x-1}=27$ là", [r"$x=2$", r"$x=1$", r"$x=\dfrac{3}{2}$", r"$x=-2$"],
              math=[r"3^{2x-1}=27", r"x=\frac{3}{2}"], vi=["Nghiệm của phương trình"]),
            Q("I", "6", "mcq", r"Trong không gian $Oxyz$, cho $\vec{a}=(1;-2;3)$ và $\vec{b}=(2;1;0)$. Tích vô hướng $\vec{a}\cdot\vec{b}$ bằng",
              [r"$0$", r"$4$", r"$-1$", r"$2$"],
              math=[r"\vec{a}=(1;-2;3)", r"\vec{b}=(2;1;0)", r"\vec{a}\cdot\vec{b}"], vi=["Trong không gian", "Tích vô hướng"]),
        ]),
        ("II", "PHẦN II. Câu trắc nghiệm đúng sai", [
            Q("II", "1", "truefalse", r"Cho hàm số $f(x)=x^3-3x^2+2$.",
              statements=[r"$f'(x)=3x^2-6x$.", r"Hàm số đồng biến trên khoảng $(0;2)$.", r"Giá trị cực đại của hàm số bằng $2$.",
                          r"$\displaystyle\int_{0}^{2}f(x)\,dx=0$."],
              math=[r"f(x)=x^3-3x^2+2", r"f'(x)=3x^2-6x", r"\int_{0}^{2}f(x)dx=0"], vi=["đồng biến trên khoảng", "Giá trị cực đại"]),
        ]),
        ("III", "PHẦN III. Câu trắc nghiệm trả lời ngắn", [
            Q("III", "1", "short", r"Tính tổng $S=\displaystyle\sum_{k=1}^{10}(2k-1)$.", math=[r"\sum_{k=1}^{10}(2k-1)"], vi=["Tính tổng"]),
            Q("III", "2", "short", r"Cho $\log_a b=2$ và $\log_a c=3$. Tính $P=\log_a\dfrac{a^2b^3}{\sqrt{c}}$.",
              math=[r"\log_ab=2", r"\log_ac=3", r"\log_a\frac{a^2b^3}{\sqrt{c}}"], vi=["Tính"]),
        ]),
        ("IV", "PHẦN IV. Tự luận", [
            Q("IV", "1", "essay", r"<i>(1,0 điểm)</i> Tìm tham số $m$ để hàm số $y=\dfrac{1}{3}x^3-mx^2+(m^2-1)x+2$ đạt cực tiểu tại $x=1$.",
              math=[r"y=\frac{1}{3}x^3-mx^2+(m^2-1)x+2"], vi=["Tìm tham số", "đạt cực tiểu tại"], label="Bài 1"),
        ]),
    ],
)

# ------------------------------------------------------------------ hard_2col: two-column multiple choice, small print
TWO_COL = [
    (r"Hàm số $y=(m-2)x+3$ đồng biến trên $\mathbb{R}$ khi", [r"$m>2$", r"$m<2$", r"$m\ge 2$", r"$m\ne 2$"], [r"y=(m-2)x+3", r"m\geq2"], ["đồng biến trên"]),
    (r"Tập nghiệm của phương trình $x^2-5x+6=0$ là", [r"$\{2;3\}$", r"$\{-2;-3\}$", r"$\{1;6\}$", r"$\{-1;6\}$"], [r"x^2-5x+6=0", r"\{-2;-3\}"], ["Tập nghiệm của phương trình"]),
    (r"Đồ thị hàm số $y=ax^2$ đi qua điểm $M(2;-8)$ thì $a$ bằng", [r"$-2$", r"$2$", r"$-4$", r"$4$"], [r"y=ax^2", r"M(2;-8)"], ["Đồ thị hàm số", "đi qua điểm"]),
    (r"Tổng hai nghiệm của phương trình $2x^2-7x+3=0$ là", [r"$\dfrac{7}{2}$", r"$-\dfrac{7}{2}$", r"$\dfrac{3}{2}$", r"$3$"], [r"2x^2-7x+3=0", r"-\frac{7}{2}"], ["Tổng hai nghiệm"]),
    (r"Góc nội tiếp chắn nửa đường tròn có số đo bằng", [r"$90^\circ$", r"$180^\circ$", r"$45^\circ$", r"$60^\circ$"], [r"180^\circ"], ["Góc nội tiếp chắn nửa đường tròn"]),
    (r"Diện tích hình tròn có bán kính $R=3$ cm là", [r"$9\pi$ cm²", r"$6\pi$ cm²", r"$3\pi$ cm²", r"$18\pi$ cm²"], [r"R=3", r"9\pi", r"18\pi"], ["Diện tích hình tròn có bán kính"]),
    (r"Thể tích hình nón có bán kính đáy $r=3$ và chiều cao $h=4$ là", [r"$12\pi$", r"$36\pi$", r"$16\pi$", r"$48\pi$"], [r"r=3", r"h=4", r"48\pi"], ["Thể tích hình nón", "chiều cao"]),
    (r"Hệ phương trình $\begin{cases}2x-y=1\\x+y=5\end{cases}$ có nghiệm là", [r"$(2;3)$", r"$(3;2)$", r"$(1;4)$", r"$(-2;3)$"], [r"2x-y=1", r"x+y=5", r"(-2;3)"], ["Hệ phương trình"]),
    (r"Với $x<3$, biểu thức $\sqrt{(x-3)^2}$ bằng", [r"$3-x$", r"$x-3$", r"$\pm(x-3)$", r"$(x-3)^2$"], [r"\sqrt{(x-3)^2}", r"\pm(x-3)"], ["biểu thức"]),
    (r"Kết quả rút gọn biểu thức $\dfrac{\sqrt{12}+\sqrt{27}}{\sqrt{3}}$ là", [r"$5$", r"$\sqrt{13}$", r"$13$", r"$3\sqrt{3}$"], [r"\frac{\sqrt{12}+\sqrt{27}}{\sqrt{3}}", r"3\sqrt{3}"], ["Kết quả rút gọn biểu thức"]),
    (r"Một người gửi tiết kiệm 100 triệu đồng với lãi suất 6%/năm. Sau một năm, số tiền lãi người đó nhận được là", ["6 triệu đồng", "0,6 triệu đồng", "106 triệu đồng", "60 triệu đồng"], [], ["gửi tiết kiệm 100 triệu đồng", "lãi suất", "số tiền lãi"]),
    (r"Tứ giác $ABCD$ nội tiếp đường tròn có $\widehat{A}=70^\circ$. Số đo $\widehat{C}$ là", [r"$110^\circ$", r"$70^\circ$", r"$20^\circ$", r"$90^\circ$"], [r"\widehat{A}=70^\circ", r"110^\circ"], ["nội tiếp đường tròn", "Số đo"]),
    (r"Phương trình $x^2-2(m+1)x+m^2=0$ có nghiệm kép khi", [r"$m=-\dfrac{1}{2}$", r"$m=\dfrac{1}{2}$", r"$m=-1$", r"$m=0$"], [r"x^2-2(m+1)x+m^2=0", r"m=-\frac{1}{2}"], ["có nghiệm kép khi"]),
    (r"Cho góc nhọn $\alpha$ có $\tan\alpha=\dfrac{3}{4}$. Giá trị của $\sin\alpha$ là", [r"$\dfrac{3}{5}$", r"$\dfrac{4}{5}$", r"$\dfrac{3}{4}$", r"$\dfrac{4}{3}$"], [r"\tan\alpha=\frac{3}{4}", r"\sin\alpha"], ["Cho góc nhọn", "Giá trị của"]),
    (r"Số nghiệm của phương trình $x^4-5x^2+4=0$ là", [r"$4$", r"$2$", r"$3$", r"$1$"], [r"x^4-5x^2+4=0"], ["Số nghiệm của phương trình"]),
    (r"Nghiệm của bất phương trình $-2x+5\le 1$ là", [r"$x\ge 2$", r"$x\le 2$", r"$x\ge -2$", r"$x>2$"], [r"-2x+5\leq1", r"x\geq-2"], ["Nghiệm của bất phương trình"]),
]
HARD_2COL = dict(
    id="hard_2col", file="hard_2col.jpg", kind="exam", full=True,
    head=("SỞ GIÁO DỤC VÀ ĐÀO TẠO<br>THÀNH PHỐ HỒ CHÍ MINH", "ĐỀ THI THỬ TUYỂN SINH VÀO LỚP 10<br>MÔN: TOÁN – PHẦN TRẮC NGHIỆM",
          "<i>Thời gian làm bài: 40 phút</i>"),
    title="ĐỀ THI THỬ TUYỂN SINH VÀO LỚP 10 – MÔN TOÁN",
    parts=[("", "", [Q("", str(i + 1), "mcq", t, c, math=m, vi=v) for i, (t, c, m, v) in enumerate(TWO_COL)])],
    two_col=True,
)

# ------------------------------------------------------------------ hard_table: statistics with tables
T1 = ('<table><tr><td>Điểm</td><td>5</td><td>6</td><td>7</td><td>8</td><td>9</td><td>10</td></tr>'
      '<tr><td>Số học sinh</td><td>4</td><td>6</td><td>10</td><td>12</td><td>5</td><td>3</td></tr></table>')
T2 = ('<table><tr><td>Thời gian (phút)</td><td>$[0;10)$</td><td>$[10;20)$</td><td>$[20;30)$</td><td>$[30;40)$</td></tr>'
      '<tr><td>Số học sinh</td><td>5</td><td>12</td><td>15</td><td>8</td></tr></table>')
HARD_TABLE = dict(
    id="hard_table", file="hard_table.jpg", kind="exam", full=True,
    head=("TRƯỜNG THPT LÊ QUÝ ĐÔN<br>TỔ TOÁN", "ĐỀ KIỂM TRA 45 PHÚT<br>CHƯƠNG: THỐNG KÊ – TOÁN 10", "<i>Thời gian làm bài: 45 phút</i>"),
    title="ĐỀ KIỂM TRA 45 PHÚT – CHƯƠNG THỐNG KÊ – TOÁN 10",
    parts=[
        ("I", "I. TRẮC NGHIỆM (6,0 điểm)", [
            Q("I", "1", "mcq", "Điểm kiểm tra môn Toán của 40 học sinh lớp 10A được cho trong bảng sau:" + T1 + "Mốt của mẫu số liệu trên là",
              ["$8$", "$7$", "$12$", "$10$"], math=[], vi=["Điểm kiểm tra môn Toán của 40 học sinh", "Mốt của mẫu số liệu"], figure=True),
            Q("I", "2", "mcq", "Số trung bình của mẫu số liệu ở Câu 1 (làm tròn đến hàng phần mười) là",
              ["$7{,}4$", "$7{,}5$", "$7{,}3$", "$7{,}6$"], math=["7,4", "7,6"], vi=["Số trung bình của mẫu số liệu", "làm tròn đến hàng phần mười"]),
            Q("I", "3", "mcq", "Trung vị của mẫu số liệu ở Câu 1 là", ["$7{,}5$", "$7$", "$8$", "$8{,}5$"], math=["8,5"], vi=["Trung vị của mẫu số liệu"]),
            Q("I", "4", "mcq", "Thời gian (phút) tự học ở nhà của 40 học sinh được ghi lại như sau:" + T2 + "Nhóm chứa mốt của mẫu số liệu ghép nhóm trên là",
              ["$[20;30)$", "$[10;20)$", "$[30;40)$", "$[0;10)$"], math=["[20;30)", "[30;40)"], vi=["tự học ở nhà", "Nhóm chứa mốt", "ghép nhóm"], figure=True),
        ]),
        ("II", "II. TỰ LUẬN (4,0 điểm)", [
            Q("II", "1a", "essay", "Cho mẫu số liệu: $2;\\ 4;\\ 4;\\ 5;\\ 7;\\ 8;\\ 9;\\ 11$.<br>a) <i>(1,0 điểm)</i> Tìm khoảng biến thiên và khoảng tứ phân vị $\\Delta_Q$ của mẫu số liệu.",
              math=[r"\Delta_Q"], vi=["khoảng biến thiên", "khoảng tứ phân vị"], label="Bài 1"),
            Q("II", "1b", "essay", "b) <i>(1,0 điểm)</i> Tìm giá trị ngoại lệ của mẫu số liệu (nếu có).", vi=["giá trị ngoại lệ"], label=""),
            Q("II", "2", "essay", "<i>(2,0 điểm)</i> Tính phương sai $s^2$ và độ lệch chuẩn $s$ của mẫu số liệu ở Câu 1 (làm tròn đến hàng phần trăm).",
              math=["s^2"], vi=["Tính phương sai", "độ lệch chuẩn", "làm tròn đến hàng phần trăm"], label="Bài 2"),
        ]),
    ],
)

# ------------------------------------------------------------------ hard_chem: chemistry (mhchem) with pen marks
HARD_CHEM = dict(
    id="hard_chem", file="hard_chem.jpg", kind="review", full=False,
    head=("TRƯỜNG THPT NGUYỄN TRÃI<br>TỔ HÓA – SINH", "ĐỀ CƯƠNG ÔN TẬP CUỐI HỌC KÌ I<br>MÔN: HÓA HỌC 10", ""),
    title="ĐỀ CƯƠNG ÔN TẬP CUỐI HỌC KÌ I – HÓA HỌC 10",
    parts=[
        ("A", "A. TRẮC NGHIỆM", [
            Q("A", "1", "mcq", r"Cấu hình electron của nguyên tử $\ce{Fe}$ ($Z=26$) là",
              [r"$1s^22s^22p^63s^23p^63d^64s^2$", r"$1s^22s^22p^63s^23p^64s^23d^6$", r"$1s^22s^22p^63s^23p^63d^8$", r"$1s^22s^22p^63s^23p^63d^54s^1$"],
              math=["1s^22s^22p^63s^23p^63d^64s^2", "1s^22s^22p^63s^23p^64s^23d^6", "1s^22s^22p^63s^23p^63d^8", "1s^22s^22p^63s^23p^63d^54s^1"],
              vi=["Cấu hình electron của nguyên tử"], chem=True),
            Q("A", "2", "mcq", r"Trong phản ứng $\ce{Fe + 2HCl -> FeCl2 + H2}$, chất khử là", [r"$\ce{Fe}$", r"$\ce{HCl}$", r"$\ce{FeCl2}$", r"$\ce{H2}$"],
              math=["Fe+2HCl->FeCl2+H2"], vi=["Trong phản ứng", "chất khử là"], chem=True),
            Q("A", "3", "mcq", r"Số oxi hóa của manganese trong $\ce{KMnO4}$ là", [r"$+7$", r"$+6$", r"$+4$", r"$+2$"],
              math=["KMnO4"], vi=["Số oxi hóa của manganese"], chem=True),
            Q("A", "4", "mcq", r"Cho phản ứng: $\ce{N2(g) + 3H2(g) <=> 2NH3(g)}$; $\Delta_r H^{o}_{298} = -92$ kJ. Phản ứng thuận là phản ứng",
              ["tỏa nhiệt", "thu nhiệt", "không kèm theo năng lượng", "chỉ xảy ra khi có xúc tác"],
              math=["N2(g)+3H2(g)<=>2NH3(g)", "-92"], vi=["Phản ứng thuận là phản ứng", "tỏa nhiệt", "thu nhiệt", "xúc tác"], chem=True),
            Q("A", "5", "mcq", r"Liên kết trong phân tử $\ce{NaCl}$ là liên kết", ["ion", "cộng hóa trị không cực", "cộng hóa trị có cực", "kim loại"],
              math=["NaCl"], vi=["Liên kết trong phân tử", "cộng hóa trị không cực"], chem=True),
        ]),
        ("B", "B. TỰ LUẬN", [
            Q("B", "6", "essay", r"Lập phương trình hóa học của phản ứng sau theo phương pháp thăng bằng electron: $\ce{Cu + HNO3 -> Cu(NO3)2 + NO + H2O}$",
              math=["Cu+HNO3->Cu(NO3)2+NO+H2O"], vi=["phương pháp thăng bằng electron"], chem=True, label="Câu 6"),
            Q("B", "7", "essay", r"Hòa tan hoàn toàn 5,6 gam $\ce{Fe}$ trong dung dịch $\ce{HCl}$ dư. Tính thể tích khí $\ce{H2}$ thu được ở điều kiện chuẩn (biết thể tích mol khí ở đkc là 24,79 L/mol).",
              math=["5,6", "24,79"], vi=["Hòa tan hoàn toàn", "dung dịch", "Tính thể tích khí", "thu được ở điều kiện chuẩn"], chem=True, label="Câu 7"),
        ]),
    ],
    marks=True,
)

CSS = """
body{font-family:"Times New Roman",serif;font-size:%dpx;line-height:1.45;width:%dpx;margin:26px auto;color:#111;position:relative}
.hd{display:flex;justify-content:space-between;text-align:center}.hd div{width:48%%}
h3{margin:8px 0 4px;font-size:1em}
.c4{display:grid;grid-template-columns:repeat(4,1fr);margin-left:18px}
.c2{display:grid;grid-template-columns:repeat(2,1fr);margin-left:18px}
.c1{margin-left:18px}
p{margin:4px 0} table{border-collapse:collapse;margin:4px 0 4px 18px} td{border:1px solid #000;padding:1px 8px;text-align:center}
.cols{column-count:2;column-gap:26px;column-rule:1px solid #999}
.cols .q{break-inside:avoid;margin-bottom:4px}
.pen{font-family:"Ink Free","Segoe Print","Comic Sans MS",cursive;color:#c0161b}
.circ{border:2px solid #c0161b;border-radius:50%%;padding:0 3px;margin-left:-5px}
"""


def choices_html(ch, circle=None):
    w = max(len(re.sub(r"\\[a-z]+|[${}^_]", "", c)) for c in ch)
    cls = "c4" if w <= 12 else ("c2" if w <= 30 else "c1")
    items = []
    for i, c in enumerate(ch):
        letter = "ABCD"[i] + "."
        if circle == i:
            letter = '<span class="circ">%s</span>' % letter
        items.append("<span>%s %s</span>" % (letter, c) if cls != "c1" else "<div>%s %s</div>" % (letter, c))
    return '<div class="%s">%s</div>' % (cls, "".join(items))


def page_html(doc):
    small = doc.get("two_col")
    h = ['<!doctype html><html lang="vi"><head><meta charset="utf-8"><link rel="stylesheet" href="%s/katex.min.css">' % KATEX.as_uri(),
         "<style>%s</style></head><body>" % (CSS % (14 if small else 16, 780))]
    l, r, t = doc["head"]
    h.append('<div class="hd"><div><b>%s</b></div><div><b>%s</b><br>%s</div></div>' % (l, r, t))
    if doc.get("marks"):
        h.append('<div class="pen" style="position:absolute;right:10px;top:70px;font-size:30px;transform:rotate(-8deg)">8,5đ</div>')
    if not small:
        h.append("<p>Họ và tên: ................................................ Lớp: ..........</p>")
    for label, title, qs in doc["parts"]:
        if title:
            h.append("<h3>%s</h3>" % title)
        if small:
            h.append('<p><i>Học sinh chọn một phương án đúng và ghi vào giấy làm bài.</i></p><div class="cols">')
        for i, q in enumerate(qs):
            lab = q["label"] if q["label"] is not None else "Câu %s" % q["n"]
            pre = "<b>%s.</b> " % lab if lab else ""
            body = pre + q["text"]
            if doc.get("marks") and q["n"] == "3":
                body += ' <span class="pen" style="font-size:22px">&nbsp;✓ +7</span>'
            block = "<p>%s</p>" % body
            if q["choices"]:
                block += choices_html(q["choices"], circle=(0 if doc.get("marks") and q["n"] in ("1", "2") else None))
            if q["statements"]:
                block += '<div class="c1">%s</div>' % "".join("<div>%s) %s</div>" % ("abcd"[j], s) for j, s in enumerate(q["statements"]))
            if doc.get("marks") and q["n"] == "6":
                block += '<p class="pen" style="font-size:21px;margin-left:40px">nhớ cân bằng O cuối cùng!</p>'
            h.append('<div class="q">%s</div>' % block if small else block)
        if small:
            h.append("</div>")
    h.append('<p style="text-align:center">----- HẾT -----</p>')
    h.append('<script src="%s/katex.min.js"></script><script src="%s/mhchem.min.js"></script>' % (KATEX.as_uri(), KATEX.as_uri()))
    h.append("</body></html>")
    html = "".join(h)
    tex = re.compile(r"\$(.+?)\$", re.S)
    html = tex.sub(lambda m: '<span class="tx" data-t="%s"></span>' % m.group(1).replace('"', "&quot;"), html)
    html = html.replace("</body>", "<script>document.querySelectorAll('.tx').forEach(e=>katex.render(e.dataset.t,e,{throwOnError:false}));</script></body>")
    return html


def photo(img, path, blur=0.6, rot=1.8):
    img = img.convert("RGB")
    img = ImageEnhance.Color(img).enhance(0.9)
    tint = Image.new("RGB", img.size, (250, 240, 215))
    img = Image.blend(img, tint, 0.18)
    img = img.rotate(random.uniform(-rot, rot), resample=Image.BICUBIC, expand=True, fillcolor=(120, 110, 95))
    img = img.filter(ImageFilter.GaussianBlur(blur))
    img = ImageEnhance.Brightness(img).enhance(random.uniform(0.92, 1.02))
    w, h = img.size
    scale = min(1.0, 2000 / max(w, h))
    img = img.resize((int(w * scale), int(h * scale)))
    img.save(path, "JPEG", quality=80)


def gt_doc(doc):
    qs = []
    for _, _, part in doc["parts"]:
        for q in part:
            qs.append({"key": (q["sec"] + "." if q["sec"] else "") + q["n"], "section": q["sec"], "n": q["n"], "kind": q["kind"],
                       "math": q["math"], "vi": q["vi"], "chem": q["chem"], "figure": q["figure"]})
    return {"id": doc["id"], "title": doc["title"], "kind": doc["kind"], "fullExam": doc["full"], "files": [doc["file"]],
            "questions": qs}


def M(key, kind, math=(), vi=(), sec=""):
    n = key.split(".")[-1]
    return {"key": key, "section": sec, "n": n, "kind": kind, "math": list(math), "vi": list(vi), "chem": False, "figure": False}


# Ground truth of the render.py photos, read from the rendered images.
DE1 = {"id": "de1", "title": "ĐỀ ÔN TẬP KIỂM TRA GIỮA HỌC KÌ I – MÔN: TOÁN 9 – ĐỀ SỐ 1", "kind": "exam", "fullExam": True,
       "files": ["de1_p1.jpg", "de1_p2.jpg", "de1_p3.jpg"], "questions": [
    M("I.1", "mcq", [r"\pm7"], ["Căn bậc hai số học của 49"], "I"),
    M("I.2", "mcq", [r"\sqrt{2x-6}", r"x\ge3", r"x\ge-3"], ["Điều kiện xác định của biểu thức"], "I"),
    M("I.3", "mcq", [r"\sqrt{12}\cdot\sqrt{3}", r"\sqrt{15}"], ["Kết quả của phép tính"], "I"),
    M("I.4", "mcq", [r"\sqrt{(1-\sqrt{2})^2}", r"1-\sqrt{2}", r"-1-\sqrt{2}"], ["Giá trị của biểu thức"], "I"),
    M("I.5", "mcq", [r"\frac{2}{\sqrt{5}-1}", r"\frac{\sqrt{5}+1}{2}", r"2\sqrt{5}+2"], ["Trục căn thức ở mẫu"], "I"),
    M("I.6", "mcq", [r"-27", r"\pm3"], ["Căn bậc ba của", "không tồn tại"], "I"),
    M("I.7", "mcq", [r"2x-y=3", r"(2;-1)"], ["Cặp số nào sau đây là nghiệm của phương trình"], "I"),
    M("I.8", "mcq", [r"x+y=5", r"x-y=1", r"(3;2)"], ["Hệ phương trình", "có nghiệm là"], "I"),
    M("I.9", "mcq", [r"3x-6>0", r"x>-2"], ["Nghiệm của bất phương trình"], "I"),
    M("I.10", "mcq", [r"\frac{x+1}{x-2}=\frac{3}{x-2}", r"x\ne2"], ["có điều kiện xác định là"], "I"),
    M("I.11", "mcq", [r"AB=3", r"AC=4", r"\frac{3}{5}", r"\frac{4}{3}"], ["vuông tại"], "I"),
    M("I.12", "mcq", [r"\cos\alpha=\frac{1}{2}", r"60^\circ"], ["là góc nhọn", "Số đo góc"], "I"),
    M("II.1a", "essay", [r"B=\sqrt{18}-2\sqrt{50}+3\sqrt{8}", r"\frac{\sqrt{x}}{\sqrt{x}-2}", r"\frac{4}{x-2\sqrt{x}}", r"\frac{\sqrt{x}}{\sqrt{x}+2}"], ["Tính giá trị của"], "II"),
    M("II.1b", "essay", [], ["Rút gọn biểu thức"], "II"),
    M("II.2a", "essay", [r"3x+2y=7", r"2x-y=0"], ["Giải hệ phương trình"], "II"),
    M("II.2b", "essay", [], ["quyên góp được 180 quyển vở", "nhiều hơn lớp 9B là 20 quyển"], "II"),
    M("II.3", "essay", [r"\frac{2x-1}{3}-\frac{x+1}{2}\le1"], ["Giải bất phương trình"], "II"),
    M("II.4a", "essay", [r"BH=4", r"HC=9"], ["đường cao", "Tính độ dài"], "II"),
    M("II.4b", "essay", [r"AB^2=BH\cdot BC"], ["làm tròn đến độ", "chứng minh"], "II"),
    M("II.5", "essay", [r"x+y=1", r"P=\frac{1}{x}+\frac{4}{y}"], ["các số thực dương thỏa mãn", "giá trị nhỏ nhất"], "II"),
]}
PHIEU = {"id": "phieu", "title": "PHIẾU BÀI TẬP TUẦN 8 – TOÁN 9", "kind": "worksheet", "fullExam": False, "files": ["phieu.jpg"], "questions": [
    M("1a", "essay", [r"\sqrt{3-x}"], ["Tìm điều kiện xác định"]),
    M("1b", "essay", [r"\sqrt{\frac{1}{x-2}}"]),
    M("2a", "essay", [r"\sqrt{20}-\sqrt{45}+3\sqrt{5}"]),
    M("2b", "essay", [r"(\sqrt{3}+1)^2-2\sqrt{3}"]),
    M("3a", "essay", [r"\frac{3}{\sqrt{6}}"], ["Trục căn thức ở mẫu"]),
    M("3b", "essay", [r"\frac{4}{\sqrt{7}+\sqrt{3}}"]),
    M("4a", "essay", [r"\sqrt{x-1}=3"], ["Giải phương trình"]),
    M("4b", "essay", [r"\sqrt{4x^2-4x+1}=5"]),
    M("5a", "essay", [r"C=\frac{\sqrt{x}+1}{\sqrt{x}-3}", r"x=16"]),
    M("5b", "essay", [], ["nhận giá trị nguyên"]),
    M("6", "essay", [r"D=\frac{1}{\sqrt{x}+1}+\frac{1}{\sqrt{x}-1}-\frac{2\sqrt{x}}{x-1}", r"x\ne1"], ["Rút gọn"]),
]}
PAGES = [
    {"file": "de1_p1.jpg", "doc": "de1", "keys": ["I.1", "I.2", "I.3", "I.4", "I.5"], "partial": ["I.6"]},
    {"file": "de1_p2.jpg", "doc": "de1", "keys": ["I.6", "I.7", "I.8", "I.9", "I.10", "I.11", "I.12"], "partial": ["II.1a"]},
    {"file": "de1_p3.jpg", "doc": "de1", "keys": ["II.1a", "II.1b", "II.2a", "II.2b", "II.3", "II.4a", "II.4b", "II.5"], "partial": []},
    {"file": "phieu.jpg", "doc": "phieu", "keys": [q["key"] for q in PHIEU["questions"]], "partial": []},
]

with sync_playwright() as p:
    b = p.chromium.launch(executable_path=CHROME) if CHROME else p.chromium.launch()
    pg = b.new_page(viewport={"width": 840, "height": 1000}, device_scale_factor=2)
    docs = [DE1, PHIEU]
    for doc, blur, rot in ((HARD_MATH, 0.6, 1.8), (HARD_2COL, 0.7, 1.2), (HARD_TABLE, 0.6, 1.8), (HARD_CHEM, 0.9, 2.4)):
        html = OUT / ("_" + doc["id"] + ".html")
        html.write_text(page_html(doc), encoding="utf-8")
        pg.goto(html.as_uri())
        pg.wait_for_timeout(900)
        png = OUT / ("_" + doc["id"] + ".png")
        pg.screenshot(path=str(png), full_page=True)
        photo(Image.open(png), OUT / doc["file"], blur, rot)
        print(OUT / doc["file"])
        g = gt_doc(doc)
        docs.append(g)
        PAGES.append({"file": doc["file"], "doc": doc["id"], "keys": [q["key"] for q in g["questions"]], "partial": []})
    b.close()

gt = {"version": 1, "note": "see hard_pages.py", "docs": docs, "pages": PAGES}
(HERE / "ground_truth.json").write_text(json.dumps(gt, ensure_ascii=False, indent=1), encoding="utf-8")
print(HERE / "ground_truth.json", sum(len(d["questions"]) for d in docs), "questions")
