# Hướng dẫn nhanh — Beluga Live

## 1. Lấy IPA

1. Mở https://github.com/thanhvu1554/beluga-live/actions, chọn lần chạy **iOS** mới nhất có dấu xanh.
2. Kéo xuống mục **Artifacts**, tải `BelugaLive-unsigned` (file zip, bên trong là `BelugaLive-unsigned.ipa`).
   Hoặc trên PC: `gh run download -R thanhvu1554/beluga-live -n BelugaLive-unsigned`
3. Artifact `screenshots` chứa ảnh chụp simulator của từng trạng thái (sáng và tối).

IPA này chưa ký, cấu trúc `Payload/BelugaLive.app`, không kèm provisioning profile.

## 2. Ký bằng gbox

1. Chuyển file `.ipa` sang iPhone (AirDrop, iCloud Drive, hoặc tải thẳng từ GitHub bằng Safari).
2. Mở bằng gbox, chọn chứng chỉ `.p12` của bạn rồi ký và cài.
3. Bundle ID mặc định là `com.thanhvu.belugalive`. Muốn đổi thì đổi trong gbox lúc ký, hoặc sửa
   một dòng `PRODUCT_BUNDLE_IDENTIFIER` trong `ios/Config/App.xcconfig` rồi để CI build lại.
4. Lần đầu mở app: cho phép camera, micro, nhận dạng giọng nói và mạng nội bộ.

## 3. Chạy server trên PC

```powershell
cd C:\Users\Beluga\Desktop\Liveapp
.\scripts\start-server.ps1 -Build     # lần đầu: build bản release rồi chạy
.\scripts\start-server.ps1            # các lần sau
.\scripts\start-server.ps1 -Mock      # chạy thử với "Claude giả", không tốn hạn mức
```

- Server in ra **địa chỉ Tailscale** và **mã ghép đôi 6 số**.
- Trong app: Cài đặt → nhập địa chỉ PC (nếu chưa điền sẵn) và mã ghép đôi. Chỉ cần nhập một lần.
- PC phải bật, không ngủ, Tailscale đang chạy trên cả PC và iPhone. Cổng 8787.
- Tự chạy khi đăng nhập Windows: `.\scripts\install-autostart.ps1` (gỡ: `uninstall-autostart.ps1`).

## 4. Giọng nói

- Key đặt trong `server/.env` (không bao giờ commit): `GENVOICE_API_KEY=...`. Có key này thì server đọc
  bằng giọng "Chi" và nghe bằng GenVoice. Mỗi ký tự đọc tốn 0,5 credit.
- Hết credit hoặc GenVoice lỗi: iPhone tự nghe và tự đọc bằng giọng hệ thống. Nên vào Cài đặt iOS →
  Trợ năng → Nội dung được đọc → Giọng nói → Tiếng Việt, tải một giọng chất lượng cao.
- Đổi giọng: trong app, Cài đặt → Giọng gia sư, bấm nghe thử rồi chọn (8 giọng nam/nữ, Bắc/Trung/Nam).
  Nghe thử phát clip có sẵn, không tốn credit. Giọng đã chọn được server nhớ, dùng cho mọi phiên.
  `GENVOICE_VOICE_ID` trong `server/.env` chỉ là giọng mặc định khi chưa chọn.
- Thêm giọng: thêm một dòng vào `server/voices.json` (id lấy từ thư viện giọng của GenVoice), rồi chạy
  `beluga-server gen-voice-previews` (tạo clip còn thiếu trong `server/voices/`, mỗi giọng khoảng 45 credit),
  rồi `beluga-server gen-voice-fillers` (8 câu chờ/nhấc máy của giọng mới, khoảng 150 credit), khởi động lại server.
- Ép dùng giọng iOS: `TTS_PROVIDER=device`, `STT_PROVIDER=device`.

## 4b. Điều khiển iPhone từ Claude Code trên PC (tùy chọn)

Chạy `.\scripts\register-mcp.ps1` một lần để đăng ký MCP `beluga` vào Claude Code của bạn. Sau đó
phiên Claude Code bất kỳ có các tool: xem trạng thái điện thoại, chụp ảnh, cho điện thoại nói, chiếu
nội dung lên bảng, nghe câu bạn nói. Server phải đang chạy.

## 5. Khi có trục trặc

| Hiện tượng | Xem gì |
|---|---|
| Pill báo mất kết nối | Server có đang chạy không, Tailscale hai bên, địa chỉ trong Cài đặt |
| Báo sai mã ghép đôi | Nhập lại mã server in ra lúc khởi động (lưu ở `server/data/pair.json`) |
| Báo hết hạn mức | Gói Claude Pro đã chạm giới hạn, chờ tới giờ đặt lại; có thể đổi model sang Haiku trong Cài đặt |
| Chờ lâu mới nói | Bình thường mất vài giây mỗi lượt; lượt có chụp ảnh lâu hơn |

Tiến độ và các quyết định kỹ thuật: xem `NOTES.md`.
