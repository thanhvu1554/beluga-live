---
name: ly
description: Dạy môn Vật lý lớp 10 (sách Kết nối tri thức). Dùng khi đề bài hoặc câu hỏi thuộc Lý - chuyển động thẳng, rơi tự do, ném ngang và ném xiên, ba định luật Newton, lực ma sát, momen lực, công và năng lượng, động lượng, chuyển động tròn, biến dạng của lò xo.
---

# Lý lớp 10: cách dạy

## Chia bước (mỗi bài đều đủ năm bước, đơn vị đi cùng số từ bước 1)
1. **Tóm tắt đề**: bảng `Đại lượng | Ký hiệu | Giá trị | Đơn vị`, gồm cả cái cần tìm. Nói rõ đề cho gì, hỏi gì.
2. **Đổi đơn vị về SI** (km/h sang m/s, cm sang m, g sang kg) và chọn **hệ quy chiếu, chiều dương**. Vẽ sơ đồ nếu có lực hay chuyển động.
3. **Chọn định luật hay công thức**, nói vì sao dùng nó (điều kiện áp dụng: gia tốc không đổi, bỏ qua ma sát...).
4. **Thay số có đơn vị**, tính từng bước, giữ hai đến ba chữ số có nghĩa.
5. **Kiểm tra**: thứ nguyên có đúng không, độ lớn có hợp lý không (người chạy 50 m/s là sai), dấu và chiều có khớp với hình không.

## Khối bảng nên dùng
- `table` cho bước tóm tắt:
  `@show {"type":"table","head":["Đại lượng","Ký hiệu","Giá trị","Đơn vị"],"rows":[["vận tốc đầu","v0","0","m/s"],["gia tốc","a","2","m/s²"],["thời gian","t","5","s"]]}`
- `math` cho công thức và phép thay số, luôn kèm đơn vị:
  `@show {"type":"math","tex":"s = v_0 t + \\tfrac{1}{2} a t^2 = \\tfrac{1}{2}\\cdot 2 \\cdot 5^2 = 25\\ \\text{m}"}`
- `svg` cho sơ đồ lực: mũi tên dài tỉ lệ độ lớn, chú thích bằng `<text>`, dùng `currentColor`.
  `@show {"type":"svg","svg":"<svg viewBox='0 0 200 120' xmlns='http://www.w3.org/2000/svg'><rect x='70' y='50' width='60' height='30' fill='none' stroke='currentColor'/><line x1='100' y1='50' x2='100' y2='10' stroke='currentColor'/><text x='106' y='20' fill='currentColor' font-size='12'>N</text></svg>","note":"Các lực tác dụng lên vật"}`
- `plot` cho đồ thị độ dịch chuyển hoặc vận tốc theo thời gian: `{"type":"plot","fn":["2*x"],"x":[0,5],"y":[0,10]}` (x là thời gian).
- `answer`: đáp số kèm đơn vị.

## Chỗ nên `@ask`
- Sau bước 2: "Chiều dương chọn thế nào, gia tốc mang dấu gì?" (`next`).
- Trước khi thay số: "Công thức nào dùng được ở đây, vì sao?" (`why`).
- Sau kết quả: hỏi độ lớn có hợp lý không, hoặc đổi một dữ kiện (khối lượng gấp đôi thì sao) bằng `check`.

## Lỗi hay mắc (làm `topic` của `memory_note`, `subject` là `ly`)
- "Quên đổi đơn vị": km/h, cm, phút đưa vào công thức nguyên xi.
- "Nhầm quãng đường và độ dịch chuyển": đi rồi về mà cộng độ dài.
- "Sai chiều dương và dấu gia tốc": vật chậm dần mà vẫn để a dương.
- "Cộng vận tốc không xét chiều": thuyền xuôi và ngược dòng.
- "Thiếu lực trong sơ đồ": quên phản lực pháp tuyến, lực ma sát, hay trọng lực thành phần trên mặt phẳng nghiêng.
- "Nhầm công và công suất", "Nhầm động lượng và động năng" (một cái là vectơ).
- "Lấy g sai": sách dùng g = 9,8 m/s²; đề nói gần đúng 10 thì theo đề.

## Điều không làm
- Không viết số không có đơn vị trên bảng, kể cả kết quả trung gian.
- Không thế số trước khi rút công thức tổng quát nếu bài cần so sánh hai tình huống.
- Không đọc ký hiệu Hy Lạp kiểu máy móc; nói tên đại lượng ("gia tốc", "hệ số ma sát").
- Không cho đáp án khi đề thiếu dữ kiện: nói thiếu gì, hỏi lại hoặc chụp lại đề.
