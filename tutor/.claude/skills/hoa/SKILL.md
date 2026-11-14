---
name: hoa
description: Dạy môn Hóa học lớp 10 (sách Kết nối tri thức). Dùng khi đề bài hoặc câu hỏi thuộc Hóa - cấu tạo nguyên tử, bảng tuần hoàn, liên kết hóa học, phản ứng oxi hóa khử, năng lượng hóa học, tốc độ phản ứng, nhóm halogen, tính mol và cân bằng phương trình.
---

# Hóa lớp 10: cách dạy

## Chia bước
1. **Viết phương trình** bằng khối `chem` ngay khi biết chất tham gia và sản phẩm, chưa cân bằng cũng được.
2. **Xác định loại bài**: cân bằng đơn giản, oxi hóa khử, tính theo mol, hay cấu hình electron. Nói tên loại để bạn ấy biết đang dùng công cụ nào.
3. **Làm từng bước kèm đơn vị và điều kiện** (khí ở đktc hay điều kiện chuẩn, hiệu suất, chất dư hay chất hết).
4. **Cân bằng và kiểm tra**: đếm nguyên tử hai vế, kiểm tra điện tích với phương trình ion.
5. **Kết luận** bằng `answer`: số mol, khối lượng, thể tích, nồng độ, kèm đơn vị.

## Khối bảng nên dùng
- `chem` cho phương trình (cú pháp mhchem, không bọc trong `\ce`):
  `@show {"type":"chem","tex":"2Al + 3Cl2 -> 2AlCl3","note":"Chưa kiểm tra điện tích hai vế"}`
  Ion: `@show {"type":"chem","tex":"Zn + Cu^2+ -> Zn^2+ + Cu"}`
  Thuận nghịch: `@show {"type":"chem","tex":"N2 + 3H2 <=> 2NH3"}`
- `table` cho số oxi hóa trước và sau, hoặc bảng mol (chất, M, m, n):
  `@show {"type":"table","head":["Nguyên tố","Trước","Sau","Thay đổi"],"rows":[["Zn","0","+2","nhường 2e (chất khử)"],["Cu","+2","0","nhận 2e (chất oxi hóa)"]]}`
- `math` cho công thức tính, đơn vị trong `\\text{}`:
  `@show {"type":"math","tex":"n = \\dfrac{m}{M} = \\dfrac{5{,}6}{56} = 0{,}1\\ \\text{mol}"}`
  Nhiệt phản ứng: `@show {"type":"math","tex":"\\Delta_r H^\\circ_{298} = \\sum \\Delta_f H^\\circ_{298}(sp) - \\sum \\Delta_f H^\\circ_{298}(cd)"}`
- `text` cho cấu hình electron và quy tắc:
  `@show {"type":"text","md":"1s² 2s² 2p⁶ 3s² 3p⁵ → **Cl**, nhóm VIIA, chu kỳ 3"}`
- `svg` cho sơ đồ liên kết hay mô hình nguyên tử đơn giản, dùng `currentColor`.

## Chỗ nên `@ask`
- Sau khi liệt kê chất: "Chất nào là chất khử, vì sao?" (`why`).
- Trước khi tính mol: "Muốn tính số mol cần biết gì?" (`next`).
- Bài chất dư: để bạn ấy tự nói chất nào hết trước khi tớ chỉ ra (`check`).
- Cuối bài: đổi một chất hay một số liệu rồi hỏi kết quả đổi thế nào (`check`).

## Lỗi hay mắc (làm `topic` của `memory_note`, `subject` là `hoa`)
- "Cân bằng phương trình": đếm sai nguyên tử, quên hệ số trước chất đơn.
- "Xác định số oxi hóa": nhầm với hóa trị, quên quy tắc tổng bằng điện tích.
- "Nhầm chất khử và chất oxi hóa": chất khử là chất nhường electron (số oxi hóa tăng).
- "Chất dư và chất hết": tính theo chất dư thay vì chất hết.
- "Đổi mol sang gam": dùng sai khối lượng mol, quên đơn vị g/mol.
- "Thể tích khí": quên điều kiện (24,79 L/mol ở điều kiện chuẩn theo sách).
- "Cấu hình electron": sai thứ tự mức năng lượng, quên 4s được điền trước 3d.
- "Nhầm loại liên kết": ion, cộng hóa trị phân cực, không phân cực (nhìn hiệu độ âm điện).

## Điều không làm
- Không cân bằng hộ cả phương trình dài; cho bạn ấy đếm từng nguyên tố trước.
- Không đọc phương trình từng ký tự; chiếu `chem` rồi nói tên chất ("axit sunfuric", "kẽm").
- Không dùng số liệu nhớ mơ hồ (độ âm điện, enthalpy tạo thành): nếu đề không cho thì nói rõ là lấy từ bảng của sách và nhờ bạn ấy chụp bảng.
- Không bỏ đơn vị ở bất kỳ dòng tính nào.
