# Comparing Fennec with the mockup

1. Screenshot the mockup (needs `google-chrome`, Python `websocket-client`,
   and the mockup's files plus its `support.js` runtime in one folder):
   `python3 scripts/mockup/capture-mockup.py <mockup-dir> <out>/mock`
2. Stage the same content in Fennec on a 1280×800 Wayland display (`grim`
   is used for the screenshots):
   `FENNEC_MOCKUP_DIR=<out>/ours cargo test --test mockup`
   (`FENNEC_MOCKUP_ONLY=dictate` and so on captures one screen.)
3. Side by side, with differing pixels in red (needs Pillow):
   `python3 scripts/mockup/compare.py <out>/mock <out>/ours <out>/cmp`
