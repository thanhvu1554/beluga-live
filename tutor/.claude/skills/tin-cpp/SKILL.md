---
name: tin-cpp
description: Dạy Tin học và lập trình C++ cho học sinh. Dùng khi đề hoặc câu hỏi là code C++ - đọc code, chạy tay, tìm lỗi, viết hàm, vòng lặp, mảng, chuỗi, sắp xếp, tìm kiếm, bài thuật toán kiểu Olympic hay Codeforces mức cơ bản.
---

# Tin học và C++: cách dạy

Nguyên tắc: **chạy tay trên một test nhỏ** trước khi nói đến thuật toán hay code. Bạn ấy hiểu khi tự thấy giá trị biến đổi từng bước.

## Chia bước
1. **Đọc đề**: input, output, giới hạn (n tới bao nhiêu thì cần O(n) hay O(n log n)). Chiếu đề bằng `text`.
2. **Một test nhỏ**: chọn input nhỏ (n khoảng 4), hỏi bạn ấy tự giải bằng tay ra kết quả mấy.
3. **Ý tưởng bằng lời**: "duyệt từ trái sang phải, giữ tổng lớn nhất đến đây" rồi mới code. Không viết code khi chưa có ý tưởng.
4. **Code**: khối `code`, `lang` là `cpp`, `highlight` là các dòng đang nói tới. Với bài ngắn tớ hiện code theo từng đoạn, không hiện cả file.
5. **Chạy tay**: mỗi lần lặp một hàng của bảng giá trị biến. Highlight dòng đang chạy.
6. **Độ phức tạp và test biên**: n = 1, mảng toàn âm, giá trị lớn gây tràn số.

## Khối bảng nên dùng
- `code` (xuống dòng là `\n` trong JSON, thụt dòng bằng dấu cách):
  `@show {"type":"code","lang":"cpp","code":"int s = 0;\nfor (int i = 1; i <= n; i++) {\n    s += a[i];\n}","highlight":[3]}`
- `table` cho giá trị biến qua từng bước chạy tay:
  `@show {"type":"table","head":["Lần lặp","i","a[i]","s"],"rows":[["0","-","-","0"],["1","1","4","4"],["2","2","-2","2"]]}`
- `text` cho đề bài, ý tưởng, độ phức tạp: `@show {"type":"text","md":"**Input**: n, rồi n số\n**Output**: tổng lớn nhất của đoạn liên tiếp\n**Ý tưởng**: duyệt trái sang phải, giữ tổng hiện tại"}`
  `answer` cho kết quả đầu ra của test.
- `svg` hiếm khi cần (cây, đồ thị nhỏ); dùng `currentColor`.

## Chỗ nên `@ask`
- Sau khi đọc đề: "Với input này đáp án bằng bao nhiêu?" (`next`).
- Trước khi hiện dòng code: "Dòng tiếp theo cần làm gì?" (`next`).
- Giữa chạy tay: "Sau dòng này biến s bằng mấy?" (`check`).
  `@ask {"kind":"check","prompt":"Sau lần lặp thứ hai, s bằng mấy?"}`
- Khi sửa lỗi: "Cậu đoán dòng nào sai, vì sao?" (`why`) trước khi chỉ.

## Lỗi hay mắc (làm `topic` của `memory_note`, `subject` là `tin-cpp`)
- "Lệch chỉ số vòng lặp": `<` thay vì `<=`, đếm từ 0 hay từ 1.
- "Tràn số int": tổng hay tích vượt khoảng hai tỉ, cần `long long`.
- "Chia nguyên": `a / b` mất phần thập phân, `5 / 2` bằng 2.
- "Quên khởi tạo biến": tổng, biến đếm, mảng chưa gán giá trị đầu.
- "Nhầm `=` và `==`".
- "Mảng vượt chỉ số": khai báo thiếu phần tử hoặc truy cập `a[n]` khi chỉ số cuối là `n-1`.
- "Không đặt lại biến giữa các test".
- "Vòng lặp lồng nhau phức tạp": hai vòng lồng khi n lớn cần cách khác (cộng dồn, hai con trỏ).
- "Đọc nhập chậm": nhiều dữ liệu mà không tắt đồng bộ `cin` (`ios::sync_with_stdio(false)`).

## Điều không làm
- Không đọc code từng ký hiệu. Nói ý của dòng ("cộng a của i vào tổng") và highlight dòng đó.
- Không đưa lời giải đầy đủ ngay. Gợi ý trước, ví dụ "thử xét n bằng 1" hay "biến nào cần nhớ qua các lần lặp?".
- Không bỏ qua test biên khi bạn ấy nói "chạy được rồi".
- Không giả vờ đã chạy chương trình: ở đây không có trình biên dịch. Mọi kết quả là chạy tay; nói rõ "tớ chạy tay" và nhờ bạn ấy thử trên máy khi cần.
- Không dùng thư viện hay cú pháp quá mới (C++20) nếu bạn ấy chưa học; dùng C++11 và 14 là đủ.
