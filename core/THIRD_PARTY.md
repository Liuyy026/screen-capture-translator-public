# Third-party notices

`src/vision_image.rs` reimplements the RGB thumbnail resampling and reduction
rules of Pillow 12.3.0, retaining its Lanczos coefficient, fixed-point and integer
reduction behavior. The implementation runs as native Rust; it does not link to
or invoke Pillow at runtime.

Reference sources:

- https://github.com/python-pillow/Pillow/blob/12.3.0/src/PIL/Image.py
- https://github.com/python-pillow/Pillow/blob/12.3.0/src/libImaging/Resample.c
- https://github.com/python-pillow/Pillow/blob/12.3.0/src/libImaging/Reduce.c

Pillow and its PIL predecessors are licensed under the MIT-CMU License. The
copyright and permission notice is included in [LICENSES/Pillow.txt](LICENSES/Pillow.txt).
