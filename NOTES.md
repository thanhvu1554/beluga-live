# NOTES — quyết định và tiến độ

## Tiến độ
(cập nhật ở cuối mỗi giai đoạn)

- [ ] 1. Khung app, giao diện lofi trắng đen (sáng/tối), mock, CI ra IPA unsigned
- [ ] 2. Server PC, luồng giọng nói (mock), trình chiếu bài giảng từng bước
- [ ] 3. Điểm dừng "vặn lại"
- [ ] Để sau: memory, ôn lại, màn hình tiến độ, module môn học

## Quyết định đã chọn

- **Bố cục repo:** `ios/` (app), `server/` (Node trên PC), `shared/board/` (bộ vẽ bảng, dùng chung),
  `tutor/` (thư mục làm việc của Claude Code: prompt gia sư, sau này là skill từng môn và memory),
  `docs/PROTOCOL.md` (hợp đồng giữa các phần).
- **Runner:** GitHub có image Xcode 27 (`xcode-27-arm64`, macOS 27, simulator iOS 27.x). App đặt
  tối thiểu iOS 26.
- **Bài giảng là văn bản theo dòng** (lời nói / `@lệnh {json}`), không dùng tool cho từng bước:
  nhanh hơn vì không tốn một vòng gọi tool cho mỗi khối. Chỉ chụp ảnh mới là tool.
- **Đồng bộ lời nói với bảng** làm ở điện thoại bằng timeline theo `seq`.
- **Bảng là gói web trong WKWebView** (KaTeX + mhchem), server cũng phục vụ gói này, nên thêm kiểu
  khối mới không phải build lại app.
- **Chưa có key ElevenLabs:** nghe bằng nhận dạng giọng nói của iOS, đọc bằng giọng hệ thống iOS.
  Điền key vào `server/.env` thì server chuyển sang ElevenLabs (chưa chạy thử với key thật).
- **Địa chỉ PC mặc định không nằm trong repo public:** đưa vào app lúc build qua secret
  `BELUGA_DEFAULT_HOST` của GitHub Actions; sửa được trong cài đặt app.
- **Claude Code không có quyền ghi:** chạy với danh sách tool tối thiểu, cwd là `tutor/`.
- **OpenCode worker:** chưa dùng ở đợt này; ba ưu tiên đầu đều là việc thiết kế và tích hợp, không
  có phần lặp lại đủ lớn để đáng viết brief.
