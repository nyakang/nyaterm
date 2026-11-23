use std::mem::MaybeUninit;

/// Single-pass copy plus opaque alpha. Each vector uses unaligned accesses;
/// decoder row padding and dirty-rectangle offsets need not be SIMD aligned.
pub(super) fn opaque_bgra(source: &[u8], output: &mut [MaybeUninit<u8>]) {
    assert_eq!(source.len(), output.len());
    assert!(source.len().is_multiple_of(4));
    let mut offset = 0;
    #[cfg(target_arch = "x86_64")]
    unsafe {
        // SSE2 is mandatory for x86-64. Only full vectors are accessed.
        use std::arch::x86_64::{_mm_loadu_si128, _mm_or_si128, _mm_set1_epi32, _mm_storeu_si128};
        let alpha = _mm_set1_epi32(0xff00_0000u32 as i32);
        while offset + 16 <= source.len() {
            let pixels = _mm_loadu_si128(source.as_ptr().add(offset).cast());
            _mm_storeu_si128(
                output.as_mut_ptr().add(offset).cast(),
                _mm_or_si128(pixels, alpha),
            );
            offset += 16;
        }
    }
    #[cfg(all(target_arch = "aarch64", target_endian = "little"))]
    unsafe {
        // Advanced SIMD is part of the supported desktop AArch64 baseline.
        use std::arch::aarch64::{vdupq_n_u32, vld1q_u8, vorrq_u8, vreinterpretq_u8_u32, vst1q_u8};
        let alpha = vreinterpretq_u8_u32(vdupq_n_u32(0xff00_0000));
        while offset + 16 <= source.len() {
            let pixels = vld1q_u8(source.as_ptr().add(offset));
            vst1q_u8(
                output.as_mut_ptr().add(offset).cast(),
                vorrq_u8(pixels, alpha),
            );
            offset += 16;
        }
    }
    for (destination, source) in output[offset..]
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .zip(source[offset..].as_chunks::<4>().0)
    {
        for index in 0..3 {
            destination[index].write(source[index]);
        }
        destination[3].write(255);
    }
}

#[cfg(test)]
mod tests {
    use crate::frame::convert::opaque_bgra;
    use std::mem::MaybeUninit;

    #[test]
    fn unaligned_vectors_and_short_tails_preserve_rgb_without_touching_guards() {
        for padding in 0..16 {
            for pixels in 0..65 {
                let source: Vec<u8> = (0..pixels * 4 + padding).map(|i| i as u8).collect();
                let mut output = vec![MaybeUninit::new(0x33); pixels * 4 + 16];
                opaque_bgra(&source[padding..], &mut output[..pixels * 4]);
                let output: Vec<u8> = output.iter().map(|v| unsafe { v.assume_init() }).collect();
                for i in 0..pixels * 4 {
                    assert_eq!(
                        output[i],
                        if i % 4 == 3 { 255 } else { source[padding + i] }
                    );
                }
                assert!(output[pixels * 4..].iter().all(|value| *value == 0x33));
            }
        }
    }
}
