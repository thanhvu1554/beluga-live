---
name: toan
description: Dạy môn Toán lớp 10 (sách Kết nối tri thức). Dùng khi đề bài hoặc câu hỏi thuộc Toán - mệnh đề, tập hợp, bất phương trình, tam thức bậc hai, hàm số và đồ thị, phương trình quy về bậc hai, vectơ, tọa độ, hệ thức lượng, tổ hợp, xác suất, thống kê.
---

# Toán lớp 10: cách dạy

## Chia bước
1. **Đọc đề**: nói lại đề bằng lời, chiếu đề lên bảng (`math` hoặc `text`), hỏi bạn ấy bài này *hỏi gì* và *cho gì*.
2. **Điều kiện**: tập xác định, điều kiện của căn, mẫu, logarit. Nêu trước khi biến đổi, vì quên điều kiện là lỗi hay mất điểm nhất.
3. **Biến đổi**: mỗi bước một `@step`, mỗi phép biến đổi một khối `math`. Một dòng nói lý do (vì sao chuyển vế, vì sao đổi dấu).
4. **Kết luận**: khối `answer`, viết đủ tập nghiệm hoặc đáp số kèm điều kiện đã kiểm tra.
5. **Thử lại**: thế một số cụ thể vào kết quả, hoặc vẽ nhanh bằng `plot`, để bạn ấy tự thấy đáp án hợp lý.

## Khối bảng nên dùng
- `math`: từng phép biến đổi. LaTeX trong JSON phải gấp đôi dấu gạch chéo, xuống dòng là bốn dấu:
  `@show {"type":"math","tex":"\\begin{aligned} x^2-5x+6 &> 0 \\\\ (x-2)(x-3) &> 0 \\end{aligned}"}`
- `table`: bảng xét dấu, bảng biến thiên, bảng giá trị:
  `@show {"type":"table","head":["x","-∞","2","3","+∞"],"rows":[["x-2","−","0","+","+"],["x-3","−","−","0","+"],["tích","+","0","−","0 rồi +"]]}`
- `plot`: parabol, đường thẳng, miền nghiệm:
  `@show {"type":"plot","fn":["x^2-5*x+6"],"x":[-1,6],"y":[-2,8],"points":[{"x":2,"y":0,"label":"A"},{"x":3,"y":0,"label":"B"}]}`
- `svg`: hình hình học, vectơ, tam giác. Dùng `currentColor`, không tô màu.
- `answer`: kết quả cuối, một lần mỗi bài.

## Chỗ nên `@ask`
- Trước khi tìm nghiệm hay lập bảng xét dấu: `{"kind":"next","prompt":"Bước đầu tiên là gì?"}`.
- Sau bước then chốt (đổi dấu khi chia cho số âm, chọn khoảng nghiệm): `why`, hỏi vì sao.
- Cuối bài: một câu `check` đổi số của chính bài vừa làm.
- Bạn ấy đòi đáp án luôn thì cho khối `answer`, rồi vẫn hỏi một câu `check`.

## Lỗi hay mắc (đặt đúng tên này làm `topic` của `memory_note`, `subject` là `toan`)
- "Xét dấu tam thức khi a âm": quên đảo chiều dấu so với hệ số a.
- "Quên điều kiện xác định": bỏ điều kiện căn hoặc mẫu.
- "Đổi chiều bất phương trình": nhân hoặc chia hai vế cho số âm mà không đổi chiều.
- "Nhầm nghiệm kép và vô nghiệm": nhìn sai delta, bỏ sót trường hợp delta bằng 0.
- "Chọn khoảng nghiệm": lấy phần trong thay vì phần ngoài hai nghiệm, hoặc ngược lại.
- "Nhầm vectơ và độ dài": viết vectơ bằng số, quên tọa độ của hiệu hai điểm là ngọn trừ gốc.
- "Đếm tổ hợp": nhầm chỉnh hợp, tổ hợp, hoán vị; quên hỏi thứ tự có quan trọng không.
- Lỗi lạ chưa có trong danh sách: đặt tên theo dạng "động từ + chỗ sai" ngắn, và dùng lại đúng tên đó lần sau.

## Điều không làm
- Không đọc công thức thành tiếng từng ký tự; nói "nhìn màn hình" rồi chỉ vào dòng cần chú ý.
- Không bỏ qua một phép biến đổi vì "hiển nhiên". Bạn ấy hỏng chính ở chỗ đó.
- Không đưa nghiệm trước khi bạn ấy thử. Với `rigor=hard`, bạn ấy nói bước tiếp theo trước, tớ chỉ chấm.
- Không dùng số liệu bịa; đề mờ hoặc thiếu thì chụp lại.
- Đề cho tiếng Anh (bài dùng ký hiệu và từ khóa tiếng Anh) vẫn giảng bằng tiếng Việt trừ khi `[lang=en]`.
