# Synthetic WeChat image fixtures

Both images are generated locally, contain no URL, session, account or token,
and do not encode a valid authorization payload. They exercise raster decoding:
41 x 41 synthetic modules, 3 white border modules, 10 pixels per module (470 px).
Dark module formula: `(x*17 + y*31 + x*y) % 7 < 3`, with conventional 7 x 7 finder
patterns at (0,0), (34,0), (0,34). PNG is grayscale; JPEG is Pillow quality 90.
The separate real-service smoke verifies that an actual image reaches the terminal;
it does not retain that image or imply successful authentication.
