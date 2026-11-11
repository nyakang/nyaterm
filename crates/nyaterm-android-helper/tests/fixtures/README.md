`black.h264` is a synthetic 16×16 black frame generated for decoder tests:

```sh
ffmpeg -f lavfi -i color=c=black:s=16x16:d=1:r=1 -frames:v 1 -c:v libx264 -preset ultrafast -tune zerolatency -f h264 black.h264
```

It contains no captured device or user data.
