//! Row padding: the one place avz knows about wgpu's 256-byte alignment.
//!
//! `copy_texture_to_buffer` requires every row to start on a
//! [`wgpu::COPY_BYTES_PER_ROW_ALIGNMENT`] boundary, so a 300-pixel-wide frame is
//! copied 1280 bytes per row and carries 80 bytes of slack the encoder must
//! never see. [`RowLayout`] owns that arithmetic and the unpadding copy;
//! nothing else in the crate mentions the alignment (`AGENTS.md`, rendering).

use crate::{Error, Result};

/// Bytes per pixel in `Rgba8UnormSrgb`, the only format avz reads back.
pub const BYTES_PER_PIXEL: u32 = 4;

/// Row stride alignment demanded by `copy_texture_to_buffer`.
const ROW_ALIGNMENT: u32 = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;

/// The largest frame dimension avz will lay out.
///
/// Far beyond any adapter's `max_texture_dimension_2d`, and low enough that
/// `width * BYTES_PER_PIXEL` cannot overflow a `u32` — which is the type wgpu
/// takes for a row stride.
const MAX_DIMENSION: u32 = 65_536;

/// How one RGBA frame is laid out in a readback buffer.
///
/// Rows in the buffer are [`RowLayout::padded_bytes_per_row`] apart; rows in the
/// frame handed to the encoder are [`RowLayout::unpadded_bytes_per_row`] apart.
/// [`RowLayout::unpad`] is the bridge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RowLayout {
    width: u32,
    height: u32,
}

impl RowLayout {
    /// Lay out a `width × height` RGBA frame.
    ///
    /// # Errors
    ///
    /// [`Error::Render`] if either dimension is zero or implausibly large. A
    /// zero-pixel frame would otherwise produce an empty copy that wgpu rejects
    /// deep inside the driver.
    pub fn new(width: u32, height: u32) -> Result<Self> {
        for (name, value) in [("width", width), ("height", height)] {
            if value == 0 {
                return Err(Error::Render(format!("frame {name} must not be zero")));
            }
            if value > MAX_DIMENSION {
                return Err(Error::Render(format!(
                    "frame {name} {value} exceeds the maximum of {MAX_DIMENSION}"
                )));
            }
        }

        Ok(Self { width, height })
    }

    /// Frame width in pixels.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Frame height in pixels.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Bytes of real pixels in one row: what the encoder consumes.
    pub fn unpadded_bytes_per_row(&self) -> u32 {
        self.width * BYTES_PER_PIXEL
    }

    /// Bytes between the starts of two rows in the readback buffer.
    ///
    /// The unpadded stride rounded up to the next 256-byte boundary.
    pub fn padded_bytes_per_row(&self) -> u32 {
        self.unpadded_bytes_per_row()
            .next_multiple_of(ROW_ALIGNMENT)
    }

    /// Slack bytes at the end of each buffer row. Zero when already aligned.
    pub fn row_padding(&self) -> u32 {
        self.padded_bytes_per_row() - self.unpadded_bytes_per_row()
    }

    /// Size of the readback buffer this layout copies into.
    pub fn buffer_size(&self) -> u64 {
        u64::from(self.padded_bytes_per_row()) * u64::from(self.height)
    }

    /// Size of the tightly packed RGBA frame this layout produces.
    pub fn frame_size(&self) -> usize {
        self.unpadded_bytes_per_row() as usize * self.height as usize
    }

    /// Copy a padded readback buffer into a tightly packed RGBA frame.
    ///
    /// # Errors
    ///
    /// [`Error::Render`] if `padded` is not [`RowLayout::buffer_size`] bytes —
    /// which would mean the mapped range and the layout disagree, and the
    /// alternative is silently emitting a skewed frame.
    pub fn unpad(&self, padded: &[u8]) -> Result<Vec<u8>> {
        let mut frame = Vec::new();
        self.unpad_into(padded, &mut frame)?;
        Ok(frame)
    }

    /// [`RowLayout::unpad`] into a caller-owned buffer, reusing its allocation.
    ///
    /// The rendering loop reads a frame every 1/fps of song; growing a fresh
    /// `Vec` each time is the one allocation worth avoiding here.
    pub fn unpad_into(&self, padded: &[u8], frame: &mut Vec<u8>) -> Result<()> {
        let expected = self.buffer_size();
        if padded.len() as u64 != expected {
            return Err(Error::Render(format!(
                "readback buffer is {} bytes, expected {expected} for a {}x{} frame",
                padded.len(),
                self.width,
                self.height,
            )));
        }

        let unpadded = self.unpadded_bytes_per_row() as usize;
        frame.clear();
        frame.reserve(self.frame_size());
        for row in padded.chunks_exact(self.padded_bytes_per_row() as usize) {
            frame.extend_from_slice(&row[..unpadded]);
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The alignment, spelled out rather than read from [`ROW_ALIGNMENT`], so a
    /// wrong constant cannot make these tests agree with a wrong implementation.
    const ALIGNMENT: usize = 256;

    fn layout(width: u32, height: u32) -> RowLayout {
        RowLayout::new(width, height).expect("a plausible frame lays out")
    }

    /// Build the buffer a `width × height` texture copy really produces: real
    /// pixels counting up per row, then padding to the next 256-byte boundary
    /// filled with a sentinel that must never survive [`RowLayout::unpad`].
    ///
    /// The stride is computed here from [`ALIGNMENT`], never from the layout
    /// under test, so an implementation that forgets to pad cannot also produce
    /// the buffer that would excuse it.
    fn padded_buffer(width: usize, height: usize) -> Vec<u8> {
        const PADDING_SENTINEL: u8 = 0xAB;

        let unpadded = width * BYTES_PER_PIXEL as usize;
        let padding = (ALIGNMENT - unpadded % ALIGNMENT) % ALIGNMENT;

        let mut buffer = Vec::with_capacity((unpadded + padding) * height);
        for row in 0..height {
            buffer.extend((0..unpadded).map(|byte| (row + byte) as u8));
            buffer.extend(std::iter::repeat_n(PADDING_SENTINEL, padding));
        }
        buffer
    }

    #[test]
    fn a_row_stride_rounds_up_to_the_256_byte_alignment() {
        // 300 px × 4 B = 1200 B, which sits between 1024 and 1280.
        let layout = layout(300, 2);

        assert_eq!(layout.unpadded_bytes_per_row(), 1200);
        assert_eq!(layout.padded_bytes_per_row(), 1280);
        assert_eq!(layout.row_padding(), 80);
        assert_eq!(layout.buffer_size(), 2560);
        assert_eq!(layout.frame_size(), 2400);
    }

    #[test]
    fn an_already_aligned_row_is_not_padded() {
        // 320 px × 4 B = 1280 B, exactly five alignment units.
        let layout = layout(320, 180);

        assert_eq!(layout.padded_bytes_per_row(), 1280);
        assert_eq!(layout.row_padding(), 0);
        assert_eq!(layout.buffer_size() as usize, layout.frame_size());
    }

    /// The behavior the whole module exists for: padding bytes are dropped and
    /// every pixel lands in the row it was rendered on.
    #[test]
    fn readback_handles_non_multiple_of_256_row_widths() {
        let layout = layout(300, 4);
        let padded = padded_buffer(300, 4);

        let frame = layout
            .unpad(&padded)
            .expect("the buffer matches the layout");

        assert_eq!(frame.len(), layout.frame_size());
        let unpadded = layout.unpadded_bytes_per_row() as usize;
        for row in 0..layout.height() as usize {
            let expected: Vec<u8> = (0..unpadded).map(|byte| (row + byte) as u8).collect();
            assert_eq!(
                &frame[row * unpadded..(row + 1) * unpadded],
                expected.as_slice(),
                "row {row} is skewed by the padding"
            );
        }
    }

    #[test]
    fn an_aligned_frame_survives_unpadding_byte_for_byte() {
        let layout = layout(64, 3);
        let padded = padded_buffer(64, 3);

        let frame = layout
            .unpad(&padded)
            .expect("the buffer matches the layout");

        assert_eq!(frame, padded, "an aligned buffer needs no rewriting");
    }

    #[test]
    fn a_buffer_that_is_not_the_padded_size_is_a_render_error() {
        let layout = layout(300, 2);
        let mut padded = padded_buffer(300, 2);
        padded.truncate(layout.frame_size());

        let err = layout
            .unpad(&padded)
            .expect_err("an unpadded buffer is not a readback buffer");

        assert!(matches!(err, Error::Render(_)), "got {err:?}");
        assert!(err.to_string().contains("2560"), "{err}");
    }

    #[test]
    fn unpadding_into_a_buffer_replaces_whatever_it_held() {
        let layout = layout(300, 2);
        let padded = padded_buffer(300, 2);
        let mut frame = vec![0xFF; 7];

        layout.unpad_into(&padded, &mut frame).expect("unpads");

        assert_eq!(frame.len(), layout.frame_size());
        assert_eq!(frame[0], 0, "the stale bytes are gone");
    }

    #[test]
    fn a_zero_dimension_frame_is_a_render_error() {
        for (width, height) in [(0, 1080), (1920, 0), (0, 0)] {
            let err = RowLayout::new(width, height).expect_err("a zero-pixel frame has no rows");
            assert!(matches!(err, Error::Render(_)), "got {err:?}");
        }
    }

    /// A stride is a `u32` in wgpu's API, so an absurd width must be refused at
    /// layout time rather than wrapping around inside a multiplication.
    #[test]
    fn an_implausible_dimension_is_a_render_error_not_an_overflow() {
        let err = RowLayout::new(u32::MAX, 1).expect_err("no adapter renders that");

        assert!(matches!(err, Error::Render(_)), "got {err:?}");
        assert!(err.to_string().contains("65536"), "{err}");
    }
}
