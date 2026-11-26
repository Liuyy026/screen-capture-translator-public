"""Bound the CPU workspace used by the final RGB projection of the LaMa model."""
import torch
from torch.utils._python_dispatch import TorchDispatchMode


class BoundedRGBConvolution(TorchDispatchMode):
    # At the model's maximum width of 1024, a 32-row im2col buffer is 392 MiB.
    # Keep the full width and an aligned row count to preserve the CPU GEMM layout.
    rows = 32

    def __torch_dispatch__(self, func, types, args=(), kwargs=None):
        kwargs = kwargs or {}
        if (func not in (torch.ops.aten._convolution.default, torch.ops.aten.convolution.default)
                or kwargs or len(args) < 9):
            return func(*args, **kwargs)

        image, weight, bias, stride, padding, dilation, transposed, output_padding, groups, *flags = args
        if (tuple(weight.shape) != (3, 64, 7, 7) or image.device.type != 'cpu'
                or image.dtype != torch.float32 or tuple(stride) != (1, 1) or tuple(padding) != (0, 0)
                or tuple(dilation) != (1, 1) or transposed or groups != 1):
            return func(*args, **kwargs)
        height, width = image.shape[-2] - 6, image.shape[-1] - 6
        if height <= self.rows or width <= 0:
            return func(*args, **kwargs)

        output = image.new_empty((image.shape[0], weight.shape[0], height, width))
        for start in range(0, height, self.rows):
            end = min(start + self.rows, height)
            # The model already applied its reflection padding. Each valid 7x7
            # convolution reads six more rows, including across an interior seam.
            band = func(image[:, :, start:end + 6, :], weight, bias, stride, padding, dilation,
                        transposed, output_padding, groups, *flags)
            output[:, :, start:end, :].copy_(band)
        return output
