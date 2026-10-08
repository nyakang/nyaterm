//! Cell graphics use shared, snapped physical-pixel edges rather than font ink bounds.

use gpui::{
    App, Bounds, ContentMask, PaintQuad, TransformationMatrix, Window, fill, point, px, rgb, size,
};

use crate::types::TerminalPaintGeometry;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CellGlyph {
    pub col: usize,
    pub ch: char,
    pub color: u32,
}

pub(super) fn is_cell_glyph(ch: char) -> bool {
    ('\u{2580}'..='\u{259f}').contains(&ch) || box_arms(ch).is_some()
}

pub(super) fn push_cell_background(
    start: usize,
    end: usize,
    color: u32,
    row: usize,
    geometry: TerminalPaintGeometry,
    scale: f32,
    out: &mut Vec<PaintQuad>,
) {
    let rect = cell_rect(start, end, row, geometry, scale);
    push_rect(rect, color, scale, out);
}

fn cell_rect(
    start: usize,
    end: usize,
    row: usize,
    geometry: TerminalPaintGeometry,
    scale: f32,
) -> Rect {
    let x = f32::from(geometry.bounds.left());
    let y = f32::from(geometry.bounds.top()) + geometry.visual_y_offset;
    Rect {
        left: ((x + start as f32 * geometry.cell_width) * scale).round(),
        right: ((x + end as f32 * geometry.cell_width) * scale).round(),
        top: ((y + row as f32 * geometry.cell_height) * scale).round(),
        bottom: ((y + (row + 1) as f32 * geometry.cell_height) * scale).round(),
    }
}

fn push_rect(rect: Rect, color: u32, scale: f32, out: &mut Vec<PaintQuad>) {
    if rect.right > rect.left && rect.bottom > rect.top {
        out.push(fill(
            Bounds::new(
                point(px(rect.left / scale), px(rect.top / scale)),
                size(
                    px((rect.right - rect.left) / scale),
                    px((rect.bottom - rect.top) / scale),
                ),
            ),
            rgb(color),
        ));
    }
}

// Coordinates here are physical pixels; conversion to GPUI logical units happens
// only after all shared edges and stroke widths have been rounded.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Rect {
    left: f32,
    top: f32,
    right: f32,
    bottom: f32,
}

impl Rect {
    fn part(self, left: f32, top: f32, right: f32, bottom: f32) -> Self {
        let x = |v: f32| (self.left + (self.right - self.left) * v).round();
        let y = |v: f32| (self.top + (self.bottom - self.top) * v).round();
        Self {
            left: x(left),
            top: y(top),
            right: x(right),
            bottom: y(bottom),
        }
    }
}

fn block_rects(ch: char, cell: Rect, mut emit: impl FnMut(Rect)) {
    match ch {
        '▀' => emit(cell.part(0., 0., 1., 0.5)),
        '▁'..='█' => emit(cell.part(0., 1. - (ch as u32 - 0x2580) as f32 / 8., 1., 1.)),
        '▉'..='▏' => emit(cell.part(0., 0., (0x2590 - ch as u32) as f32 / 8., 1.)),
        '▐' => emit(cell.part(0.5, 0., 1., 1.)),
        '▔' => emit(cell.part(0., 0., 1., 0.125)),
        '▕' => emit(cell.part(0.875, 0., 1., 1.)),
        '▖'..='▟' => {
            // Bit order: upper left, upper right, lower left, lower right.
            let mask = [4, 8, 1, 13, 9, 7, 11, 2, 6, 14][ch as usize - 0x2596];
            for (i, (l, t, r, b)) in [
                (0., 0., 0.5, 0.5),
                (0.5, 0., 1., 0.5),
                (0., 0.5, 0.5, 1.),
                (0.5, 0.5, 1., 1.),
            ]
            .into_iter()
            .enumerate()
            {
                if mask & (1 << i) != 0 {
                    emit(cell.part(l, t, r, b));
                }
            }
        }
        _ => {}
    }
}

fn box_rects(arms: [u8; 4], cell: Rect, thin: f32, mut emit: impl FnMut(Rect)) {
    let width = cell.right - cell.left;
    let height = cell.bottom - cell.top;
    let heavy = (thin * 2.).min(width.min(height));
    let cx = ((cell.left + cell.right - thin) / 2.).round() + thin / 2.;
    let cy = ((cell.top + cell.bottom - thin) / 2.).round() + thin / 2.;
    for (direction, weight) in arms.into_iter().enumerate() {
        if weight == 0 {
            continue;
        }
        let horizontal = direction < 2;
        let sign = if direction == 0 || direction == 2 {
            -1.
        } else {
            1.
        };
        let lanes: &[f32] = if weight == 3 { &[-1., 1.] } else { &[0.] };
        for &lane in lanes {
            let thickness = if weight == 2 { heavy } else { thin };
            let perpendicular = if horizontal {
                [arms[2], arms[3]]
            } else {
                [arms[0], arms[1]]
            };
            let end = if weight != 3 {
                if perpendicular.contains(&3) {
                    thin + thickness / 2.
                } else {
                    thickness / 2.
                }
            } else {
                match perpendicular {
                    [0, 0] => 0.,
                    [0, 3] => -sign * lane * thin + thin / 2.,
                    [3, 0] => sign * lane * thin + thin / 2.,
                    [3, 3] if arms[direction ^ 1] == 0 => -thin + thin / 2.,
                    _ => thin + thin / 2.,
                }
            };
            let rect = if horizontal {
                let y = cy + lane * thin;
                Rect {
                    left: if sign < 0. { cell.left } else { cx - end },
                    right: if sign < 0. { cx + end } else { cell.right },
                    top: (y - thickness / 2.).round(),
                    bottom: (y - thickness / 2.).round() + thickness,
                }
            } else {
                let x = cx + lane * thin;
                Rect {
                    top: if sign < 0. { cell.top } else { cy - end },
                    bottom: if sign < 0. { cy + end } else { cell.bottom },
                    left: (x - thickness / 2.).round(),
                    right: (x - thickness / 2.).round() + thickness,
                }
            };
            emit(Rect {
                left: rect.left.max(cell.left).round(),
                top: rect.top.max(cell.top).round(),
                right: rect.right.min(cell.right).round(),
                bottom: rect.bottom.min(cell.bottom).round(),
            });
        }
    }
}

// GPUI caches these three monochrome atlas masks at a fixed physical size.
// Foreground tint, font size and DPI do not create additional texture variants.
const SHADE_TILE_SIZE: u32 = 64;

#[derive(Clone, Copy, PartialEq, Eq)]
struct ShadeStyle {
    density: u8,
    color: u32,
}

fn shade_asset(density: u8) -> (&'static str, &'static [u8]) {
    match density {
        1 => ("nyaterm/terminal/shade-light-v1", br##"<svg xmlns="http://www.w3.org/2000/svg" width="64" height="64" viewBox="0 0 64 64"><defs><pattern id="p" width="2" height="2" patternUnits="userSpaceOnUse"><path d="M0 0h1v1H0z" fill="white"/></pattern></defs><rect width="64" height="64" fill="url(#p)"/></svg>"##),
        2 => ("nyaterm/terminal/shade-medium-v1", br##"<svg xmlns="http://www.w3.org/2000/svg" width="64" height="64" viewBox="0 0 64 64"><defs><pattern id="p" width="2" height="2" patternUnits="userSpaceOnUse"><path d="M0 0h1v1H0z M1 1h1v1H1z" fill="white"/></pattern></defs><rect width="64" height="64" fill="url(#p)"/></svg>"##),
        3 => ("nyaterm/terminal/shade-dark-v1", br##"<svg xmlns="http://www.w3.org/2000/svg" width="64" height="64" viewBox="0 0 64 64"><defs><pattern id="p" width="2" height="2" patternUnits="userSpaceOnUse"><path d="M0 0h2v1H0z M1 1h1v1H1z" fill="white"/></pattern></defs><rect width="64" height="64" fill="url(#p)"/></svg>"##),
        _ => unreachable!("shade density is between 1 and 3"),
    }
}

#[derive(Default)]
pub(super) struct CellGlyphPaintPlan {
    quads: Vec<PaintQuad>,
    shades: Vec<ShadePaint>,
    shade_regions: Vec<ShadeRegion>,
    shade_row_top: Option<f32>,
    previous_shade_regions: Vec<usize>,
    current_shade_regions: Vec<usize>,
    previous_shade_cursor: usize,
}

struct ShadeRegion {
    rect: Rect,
    style: ShadeStyle,
    scale: f32,
}

struct ShadePaint {
    bounds: Bounds<gpui::Pixels>,
    image_bounds: Bounds<gpui::Pixels>,
    style: ShadeStyle,
}

impl CellGlyphPaintPlan {
    pub fn primitive_count(&self) -> usize {
        self.quads.len()
            + self.shades.len()
            + self
                .shade_regions
                .iter()
                .map(|region| {
                    let tiles = |start: f32, end: f32| {
                        ((end as i32 - 1).div_euclid(SHADE_TILE_SIZE as i32)
                            - (start as i32).div_euclid(SHADE_TILE_SIZE as i32)
                            + 1) as usize
                    };
                    tiles(region.rect.left, region.rect.right)
                        * tiles(region.rect.top, region.rect.bottom)
                })
                .sum::<usize>()
    }

    /// Tile after merging neighboring rows, so shared tiles are submitted once.
    pub fn finish(&mut self) {
        for region in self.shade_regions.drain(..) {
            tile_shade(region.rect, region.style, region.scale, &mut self.shades);
        }
        self.previous_shade_regions.clear();
        self.current_shade_regions.clear();
        self.shade_row_top = None;
        self.previous_shade_cursor = 0;
    }

    pub fn paint(&mut self, window: &mut Window, cx: &mut App) {
        self.finish();
        for quad in self.quads.drain(..) {
            window.paint_quad(quad);
        }
        for shade in self.shades.drain(..) {
            let (path, data) = shade_asset(shade.style.density);
            let clip = ContentMask {
                bounds: window.content_mask().bounds.intersect(&shade.bounds),
            };
            window.with_content_mask(Some(clip), |window| {
                let _ = window.paint_svg(
                    shade.image_bounds,
                    gpui::SharedString::new_static(path),
                    Some(data),
                    TransformationMatrix::unit(),
                    rgb(shade.style.color).into(),
                    cx,
                );
            });
        }
    }
}

fn push_shade(cell: Rect, style: ShadeStyle, scale: f32, out: &mut CellGlyphPaintPlan) {
    if cell.right <= cell.left || cell.bottom <= cell.top {
        return;
    }
    if out.shade_row_top != Some(cell.top) {
        std::mem::swap(
            &mut out.previous_shade_regions,
            &mut out.current_shade_regions,
        );
        out.current_shade_regions.clear();
        out.previous_shade_cursor = 0;
        out.shade_row_top = Some(cell.top);
    }
    // Glyph runs arrive in column order. A cursor into the previous row makes
    // matching multiple shade bands linear, without a lookup table or sorting.
    while let Some(&index) = out.previous_shade_regions.get(out.previous_shade_cursor) {
        if out.shade_regions[index].rect.right > cell.left {
            break;
        }
        out.previous_shade_cursor += 1;
    }
    let previous = out
        .previous_shade_regions
        .get(out.previous_shade_cursor)
        .copied()
        .filter(|&index| {
            let last = &out.shade_regions[index];
            last.style == style
                && last.scale == scale
                && last.rect.left == cell.left
                && last.rect.right == cell.right
                && last.rect.bottom == cell.top
        });
    let index = if let Some(index) = previous {
        out.shade_regions[index].rect.bottom = cell.bottom;
        index
    } else {
        let index = out.shade_regions.len();
        out.shade_regions.push(ShadeRegion {
            rect: cell,
            style,
            scale,
        });
        index
    };
    out.current_shade_regions.push(index);
}

fn tile_shade(cell: Rect, style: ShadeStyle, scale: f32, out: &mut Vec<ShadePaint>) {
    let tile_size = SHADE_TILE_SIZE as i32;
    let first_x = (cell.left as i32).div_euclid(tile_size) * tile_size;
    let first_y = (cell.top as i32).div_euclid(tile_size) * tile_size;
    let bounds = Bounds::new(
        point(px(cell.left / scale), px(cell.top / scale)),
        size(
            px((cell.right - cell.left) / scale),
            px((cell.bottom - cell.top) / scale),
        ),
    );
    // All tile origins have even parity, including negative scroll offsets.
    // Cropping a tile at 1:1 physical resolution preserves the former dot pattern.
    for y in (first_y..cell.bottom as i32).step_by(tile_size as usize) {
        for x in (first_x..cell.right as i32).step_by(tile_size as usize) {
            let image_bounds = Bounds::new(
                point(px(x as f32 / scale), px(y as f32 / scale)),
                size(
                    px(SHADE_TILE_SIZE as f32 / scale),
                    px(SHADE_TILE_SIZE as f32 / scale),
                ),
            );
            out.push(ShadePaint {
                bounds,
                image_bounds,
                style,
            });
        }
    }
}

fn can_merge_horizontal(ch: char) -> bool {
    matches!(ch, '\u{2580}'..='\u{2588}' | '\u{2591}'..='\u{2594}' | '─' | '━' | '═')
}

pub(super) fn push_cell_glyphs(
    glyphs: &[CellGlyph],
    row: usize,
    geometry: TerminalPaintGeometry,
    scale: f32,
    out: &mut CellGlyphPaintPlan,
) {
    let thin = (geometry.cell_width.min(geometry.cell_height) * scale / 8.)
        .round()
        .max(1.);
    let mut index = 0;
    while let Some(glyph) = glyphs.get(index) {
        let mut end = glyph.col + 1;
        index += 1;
        if can_merge_horizontal(glyph.ch) {
            while let Some(next) = glyphs
                .get(index)
                .filter(|next| next.col == end && next.ch == glyph.ch && next.color == glyph.color)
            {
                end = next.col + 1;
                index += 1;
            }
        }
        let cell = cell_rect(glyph.col, end, row, geometry, scale);
        if ('░'..='▓').contains(&glyph.ch) {
            push_shade(
                cell,
                ShadeStyle {
                    density: (glyph.ch as u32 - 0x2590) as u8,
                    color: glyph.color,
                },
                scale,
                out,
            );
        } else if let Some(arms) = box_arms(glyph.ch) {
            box_rects(arms, cell, thin, |rect| {
                push_rect(rect, glyph.color, scale, &mut out.quads)
            });
        } else {
            block_rects(glyph.ch, cell, |rect| {
                push_rect(rect, glyph.color, scale, &mut out.quads)
            });
        }
    }
}

// Solid box-drawing arms: left, right, up, down; 0=absent, 1=light,
// 2=heavy, 3=double. Dashed, curved and diagonal glyphs keep the font path.
fn box_arms(ch: char) -> Option<[u8; 4]> {
    let arms = match ch {
        '\u{2500}' => [1, 1, 0, 0], // light horizontal
        '\u{2501}' => [2, 2, 0, 0], // heavy horizontal
        '\u{2502}' => [0, 0, 1, 1], // light vertical
        '\u{2503}' => [0, 0, 2, 2], // heavy vertical
        '\u{250c}' => [0, 1, 0, 1], // light down and right
        '\u{250d}' => [0, 2, 0, 1], // down light and right heavy
        '\u{250e}' => [0, 1, 0, 2], // down heavy and right light
        '\u{250f}' => [0, 2, 0, 2], // heavy down and right
        '\u{2510}' => [1, 0, 0, 1], // light down and left
        '\u{2511}' => [2, 0, 0, 1], // down light and left heavy
        '\u{2512}' => [1, 0, 0, 2], // down heavy and left light
        '\u{2513}' => [2, 0, 0, 2], // heavy down and left
        '\u{2514}' => [0, 1, 1, 0], // light up and right
        '\u{2515}' => [0, 2, 1, 0], // up light and right heavy
        '\u{2516}' => [0, 1, 2, 0], // up heavy and right light
        '\u{2517}' => [0, 2, 2, 0], // heavy up and right
        '\u{2518}' => [1, 0, 1, 0], // light up and left
        '\u{2519}' => [2, 0, 1, 0], // up light and left heavy
        '\u{251a}' => [1, 0, 2, 0], // up heavy and left light
        '\u{251b}' => [2, 0, 2, 0], // heavy up and left
        '\u{251c}' => [0, 1, 1, 1], // light vertical and right
        '\u{251d}' => [0, 2, 1, 1], // vertical light and right heavy
        '\u{251e}' => [0, 1, 2, 1], // up heavy and right down light
        '\u{251f}' => [0, 1, 1, 2], // down heavy and right up light
        '\u{2520}' => [0, 1, 2, 2], // vertical heavy and right light
        '\u{2521}' => [0, 2, 2, 1], // down light and right up heavy
        '\u{2522}' => [0, 2, 1, 2], // up light and right down heavy
        '\u{2523}' => [0, 2, 2, 2], // heavy vertical and right
        '\u{2524}' => [1, 0, 1, 1], // light vertical and left
        '\u{2525}' => [2, 0, 1, 1], // vertical light and left heavy
        '\u{2526}' => [1, 0, 2, 1], // up heavy and left down light
        '\u{2527}' => [1, 0, 1, 2], // down heavy and left up light
        '\u{2528}' => [1, 0, 2, 2], // vertical heavy and left light
        '\u{2529}' => [2, 0, 2, 1], // down light and left up heavy
        '\u{252a}' => [2, 0, 1, 2], // up light and left down heavy
        '\u{252b}' => [2, 0, 2, 2], // heavy vertical and left
        '\u{252c}' => [1, 1, 0, 1], // light down and horizontal
        '\u{252d}' => [2, 1, 0, 1], // left heavy and right down light
        '\u{252e}' => [1, 2, 0, 1], // right heavy and left down light
        '\u{252f}' => [2, 2, 0, 1], // down light and horizontal heavy
        '\u{2530}' => [1, 1, 0, 2], // down heavy and horizontal light
        '\u{2531}' => [2, 1, 0, 2], // right light and left down heavy
        '\u{2532}' => [1, 2, 0, 2], // left light and right down heavy
        '\u{2533}' => [2, 2, 0, 2], // heavy down and horizontal
        '\u{2534}' => [1, 1, 1, 0], // light up and horizontal
        '\u{2535}' => [2, 1, 1, 0], // left heavy and right up light
        '\u{2536}' => [1, 2, 1, 0], // right heavy and left up light
        '\u{2537}' => [2, 2, 1, 0], // up light and horizontal heavy
        '\u{2538}' => [1, 1, 2, 0], // up heavy and horizontal light
        '\u{2539}' => [2, 1, 2, 0], // right light and left up heavy
        '\u{253a}' => [1, 2, 2, 0], // left light and right up heavy
        '\u{253b}' => [2, 2, 2, 0], // heavy up and horizontal
        '\u{253c}' => [1, 1, 1, 1], // light vertical and horizontal
        '\u{253d}' => [2, 1, 1, 1], // left heavy and right vertical light
        '\u{253e}' => [1, 2, 1, 1], // right heavy and left vertical light
        '\u{253f}' => [2, 2, 1, 1], // vertical light and horizontal heavy
        '\u{2540}' => [1, 1, 2, 1], // up heavy and down horizontal light
        '\u{2541}' => [1, 1, 1, 2], // down heavy and up horizontal light
        '\u{2542}' => [1, 1, 2, 2], // vertical heavy and horizontal light
        '\u{2543}' => [2, 1, 2, 1], // left up heavy and right down light
        '\u{2544}' => [1, 2, 2, 1], // right up heavy and left down light
        '\u{2545}' => [2, 1, 1, 2], // left down heavy and right up light
        '\u{2546}' => [1, 2, 1, 2], // right down heavy and left up light
        '\u{2547}' => [2, 2, 2, 1], // down light and up horizontal heavy
        '\u{2548}' => [2, 2, 1, 2], // up light and down horizontal heavy
        '\u{2549}' => [2, 1, 2, 2], // right light and left vertical heavy
        '\u{254a}' => [1, 2, 2, 2], // left light and right vertical heavy
        '\u{254b}' => [2, 2, 2, 2], // heavy vertical and horizontal
        '\u{2550}' => [3, 3, 0, 0], // double horizontal
        '\u{2551}' => [0, 0, 3, 3], // double vertical
        '\u{2552}' => [0, 3, 0, 1], // down single and right double
        '\u{2553}' => [0, 1, 0, 3], // down double and right single
        '\u{2554}' => [0, 3, 0, 3], // double down and right
        '\u{2555}' => [3, 0, 0, 1], // down single and left double
        '\u{2556}' => [1, 0, 0, 3], // down double and left single
        '\u{2557}' => [3, 0, 0, 3], // double down and left
        '\u{2558}' => [0, 3, 1, 0], // up single and right double
        '\u{2559}' => [0, 1, 3, 0], // up double and right single
        '\u{255a}' => [0, 3, 3, 0], // double up and right
        '\u{255b}' => [3, 0, 1, 0], // up single and left double
        '\u{255c}' => [1, 0, 3, 0], // up double and left single
        '\u{255d}' => [3, 0, 3, 0], // double up and left
        '\u{255e}' => [0, 3, 1, 1], // vertical single and right double
        '\u{255f}' => [0, 1, 3, 3], // vertical double and right single
        '\u{2560}' => [0, 3, 3, 3], // double vertical and right
        '\u{2561}' => [3, 0, 1, 1], // vertical single and left double
        '\u{2562}' => [1, 0, 3, 3], // vertical double and left single
        '\u{2563}' => [3, 0, 3, 3], // double vertical and left
        '\u{2564}' => [3, 3, 0, 1], // down single and horizontal double
        '\u{2565}' => [1, 1, 0, 3], // down double and horizontal single
        '\u{2566}' => [3, 3, 0, 3], // double down and horizontal
        '\u{2567}' => [3, 3, 1, 0], // up single and horizontal double
        '\u{2568}' => [1, 1, 3, 0], // up double and horizontal single
        '\u{2569}' => [3, 3, 3, 0], // double up and horizontal
        '\u{256a}' => [3, 3, 1, 1], // vertical single and horizontal double
        '\u{256b}' => [1, 1, 3, 3], // vertical double and horizontal single
        '\u{256c}' => [3, 3, 3, 3], // double vertical and horizontal
        '\u{2574}' => [1, 0, 0, 0], // light left
        '\u{2575}' => [0, 0, 1, 0], // light up
        '\u{2576}' => [0, 1, 0, 0], // light right
        '\u{2577}' => [0, 0, 0, 1], // light down
        '\u{2578}' => [2, 0, 0, 0], // heavy left
        '\u{2579}' => [0, 0, 2, 0], // heavy up
        '\u{257a}' => [0, 2, 0, 0], // heavy right
        '\u{257b}' => [0, 0, 0, 2], // heavy down
        '\u{257c}' => [1, 2, 0, 0], // light left and heavy right
        '\u{257d}' => [0, 0, 1, 2], // light up and heavy down
        '\u{257e}' => [2, 1, 0, 0], // heavy left and light right
        '\u{257f}' => [0, 0, 2, 1], // heavy up and light down
        _ => return None,
    };
    Some(arms)
}

#[cfg(test)]
mod tests {
    use super::SHADE_TILE_SIZE;
    use gpui::{Bounds, point, px, size};

    use super::{
        CellGlyph, CellGlyphPaintPlan, Rect, block_rects as emit_blocks, box_arms,
        box_rects as emit_box, cell_rect, is_cell_glyph, push_cell_glyphs, shade_asset,
    };

    fn block_rects(ch: char, cell: Rect) -> Vec<Rect> {
        let mut out = Vec::new();
        emit_blocks(ch, cell, |rect| out.push(rect));
        out
    }

    fn box_rects(arms: [u8; 4], cell: Rect, thin: f32) -> Vec<Rect> {
        let mut out = Vec::new();
        emit_box(arms, cell, thin, |rect| out.push(rect));
        out
    }
    use crate::types::TerminalPaintGeometry;

    #[test]
    fn blocks_share_edges_across_columns_rows_and_fractional_scales() {
        for scale in [1., 1.25, 1.5, 2.] {
            for width in [7.3, 9.5, 12.8] {
                let geometry = TerminalPaintGeometry {
                    bounds: Bounds::new(point(px(3.2), px(5.7)), size(px(500.), px(500.))),
                    visual_y_offset: -3.4,
                    cell_width: width,
                    cell_height: (19.3_f32 * scale).round() / scale,
                };
                for col in 0..20 {
                    let cell = cell_rect(col, col + 1, 0, geometry, scale);
                    let next = cell_rect(col + 1, col + 2, 0, geometry, scale);
                    let below = cell_rect(col, col + 1, 1, geometry, scale);
                    assert_eq!(block_rects('█', cell), vec![cell]);
                    assert_eq!(cell.right, next.left);
                    assert_eq!(cell.bottom, below.top);
                    assert_eq!(
                        block_rects('▀', cell)[0].bottom,
                        block_rects('▄', cell)[0].top
                    );
                    assert_eq!(
                        block_rects('▌', cell)[0].right,
                        block_rects('▐', cell)[0].left
                    );
                }
            }
        }
    }

    #[test]
    fn solid_block_elements_have_integer_geometry_within_the_cell() {
        let cell = Rect {
            left: 1.,
            top: 3.,
            right: 18.,
            bottom: 32.,
        };
        for code in 0x2580..=0x259f {
            let ch = char::from_u32(code).unwrap();
            assert!(is_cell_glyph(ch));
            if ('░'..='▓').contains(&ch) {
                continue;
            }
            let rects = block_rects(ch, cell);
            assert!(!rects.is_empty(), "{ch}");
            for rect in rects {
                assert!(rect.left >= cell.left && rect.right <= cell.right);
                assert!(rect.top >= cell.top && rect.bottom <= cell.bottom);
                for edge in [rect.left, rect.top, rect.right, rect.bottom] {
                    assert_eq!(edge, edge.round());
                }
            }
        }
    }

    #[test]
    fn shared_shade_masks_have_exact_density_and_no_antialiased_dot_edges() {
        use gpui::{SvgRenderer, SvgSize};
        use std::sync::Arc;

        let renderer = SvgRenderer::new(Arc::new(()));
        for density in 1..=3 {
            let (_, data) = shade_asset(density);
            let parsed = renderer.parse_svg(data).unwrap();
            // paint_svg uses a 2x mask; each physical dot becomes a 2x2 texel block.
            let image = renderer
                .render_parsed(&parsed, SvgSize::ExactSize(size(128.into(), 128.into())))
                .unwrap();
            let bytes = image.as_bytes(0).unwrap();
            let foreground = bytes
                .as_chunks::<4>()
                .0
                .iter()
                .filter(|pixel| pixel[3] == 255)
                .count();
            assert_eq!(foreground, 128 * 128 * density as usize / 4);
            for y in 0..128 {
                for x in 0..128 {
                    let rank = match ((x / 2) % 2, (y / 2) % 2) {
                        (0, 0) => 0,
                        (1, 1) => 1,
                        (1, 0) => 2,
                        _ => 3,
                    };
                    assert_eq!(
                        bytes[(y * 128 + x) * 4 + 3],
                        if rank < density as usize { 255 } else { 0 }
                    );
                }
            }
        }
    }

    fn geometry() -> TerminalPaintGeometry {
        TerminalPaintGeometry {
            bounds: Bounds::new(point(px(0.), px(0.)), size(px(800.), px(480.))),
            visual_y_offset: 0.,
            cell_width: 10.,
            cell_height: 20.,
        }
    }

    #[test]
    fn full_viewport_shades_and_blocks_have_bounded_primitive_counts() {
        for scale in [1., 1.25, 1.5, 2.] {
            for ch in ['▓', '█', '▀', '─'] {
                let glyphs = (0..80)
                    .map(|col| CellGlyph {
                        col,
                        ch,
                        color: 0x123456,
                    })
                    .collect::<Vec<_>>();
                let mut plan = CellGlyphPaintPlan::default();
                for row in 0..24 {
                    push_cell_glyphs(&glyphs, row, geometry(), scale, &mut plan);
                }
                plan.finish();
                if ch == '▓' {
                    assert!(plan.quads.is_empty());
                    let width = (800. * scale / SHADE_TILE_SIZE as f32).ceil() as usize;
                    let height = (480. * scale / SHADE_TILE_SIZE as f32).ceil() as usize;
                    assert_eq!(plan.primitive_count(), width * height);
                } else {
                    assert!(plan.shades.is_empty());
                    assert!(plan.quads.len() <= 48);
                }
            }
        }
    }

    #[test]
    fn horizontal_merging_stops_at_color_changes_and_missing_columns() {
        let glyphs = [(0, 1), (1, 1), (2, 2), (4, 2)].map(|(col, color)| CellGlyph {
            col,
            ch: '█',
            color,
        });
        let mut plan = CellGlyphPaintPlan::default();
        push_cell_glyphs(&glyphs, 0, geometry(), 1., &mut plan);
        assert_eq!(plan.quads.len(), 3);
        assert_eq!(plan.quads[0].bounds.size.width, px(20.));
        assert_eq!(plan.quads[1].bounds.origin.x, px(20.));
        assert_eq!(plan.quads[2].bounds.origin.x, px(40.));
    }

    #[test]
    fn cropped_shade_tiles_preserve_dot_phase_when_scaled_and_scrolled() {
        for scale in [1., 1.25, 1.5, 2.] {
            for offset in [-65.3, -1.7, 0., 63.5] {
                let mut geometry = geometry();
                geometry.visual_y_offset = offset;
                geometry.bounds.origin.x = px(3.7);
                let glyphs = (0..8)
                    .map(|col| CellGlyph {
                        col,
                        ch: '▒',
                        color: 0x123456,
                    })
                    .collect::<Vec<_>>();
                let mut plan = CellGlyphPaintPlan::default();
                for row in 0..3 {
                    push_cell_glyphs(&glyphs, row, geometry, scale, &mut plan);
                }
                plan.finish();
                let mut cell = cell_rect(0, 8, 0, geometry, scale);
                cell.bottom = cell_rect(0, 8, 2, geometry, scale).bottom;
                for y in cell.top as i32..cell.bottom as i32 {
                    for x in cell.left as i32..cell.right as i32 {
                        let matching = plan
                            .shades
                            .iter()
                            .filter(|shade| {
                                let visible = shade.bounds.intersect(&shade.image_bounds);
                                let left = (f32::from(visible.left()) * scale).round() as i32;
                                let top = (f32::from(visible.top()) * scale).round() as i32;
                                let right = (f32::from(visible.right()) * scale).round() as i32;
                                let bottom = (f32::from(visible.bottom()) * scale).round() as i32;
                                x >= left && x < right && y >= top && y < bottom
                            })
                            .collect::<Vec<_>>();
                        assert_eq!(matching.len(), 1);
                        let shade = matching[0];
                        let tile_x =
                            x - (f32::from(shade.image_bounds.left()) * scale).round() as i32;
                        let tile_y =
                            y - (f32::from(shade.image_bounds.top()) * scale).round() as i32;
                        // Both diagonal positions in the 2x2 tile are foreground.
                        assert_eq!(tile_x % 2 == tile_y % 2, x.rem_euclid(2) == y.rem_euclid(2));
                    }
                }
            }
        }
    }

    #[test]
    fn vertical_shade_merging_preserves_gaps_colors_densities_and_widths() {
        let mut plan = CellGlyphPaintPlan::default();
        for (row, ch, color, width) in [
            (0, '░', 1, 4),
            (1, '▒', 1, 4),
            (3, '▒', 1, 4),
            (4, '▒', 2, 4),
            (5, '▒', 2, 2),
        ] {
            let glyphs = (0..width)
                .map(|col| CellGlyph { col, ch, color })
                .collect::<Vec<_>>();
            push_cell_glyphs(&glyphs, row, geometry(), 1., &mut plan);
        }
        let before = plan.primitive_count();
        plan.finish();
        assert_eq!(plan.primitive_count(), before);
        for row in 0..6 {
            for col in 0..4 {
                let x = px(col as f32 * 10. + 5.);
                let y = px(row as f32 * 20. + 5.);
                let styles = plan
                    .shades
                    .iter()
                    .filter(|shade| {
                        shade
                            .bounds
                            .intersect(&shade.image_bounds)
                            .contains(&point(x, y))
                    })
                    .map(|shade| (shade.style.density, shade.style.color))
                    .collect::<Vec<_>>();
                let expected = match row {
                    0 => vec![(1, 1)],
                    1 | 3 => vec![(2, 1)],
                    4 => vec![(2, 2)],
                    5 if col < 2 => vec![(2, 2)],
                    _ => Vec::new(),
                };
                assert_eq!(styles, expected, "row={row}, col={col}");
            }
        }
    }

    #[test]
    fn neighboring_rows_merge_multiple_shade_bands_without_filling_between_them() {
        let glyphs = (0..8)
            .map(|col| CellGlyph {
                col,
                ch: if col < 4 { '░' } else { '▒' },
                color: 0x123456,
            })
            .collect::<Vec<_>>();
        let mut plan = CellGlyphPaintPlan::default();
        for row in 0..3 {
            push_cell_glyphs(&glyphs, row, geometry(), 1., &mut plan);
        }
        plan.finish();
        assert_eq!(plan.primitive_count(), 3);
        for y in 0..60 {
            for x in 0..80 {
                let owners = plan
                    .shades
                    .iter()
                    .filter(|shade| {
                        shade
                            .bounds
                            .intersect(&shade.image_bounds)
                            .contains(&point(px(x as f32 + 0.5), px(y as f32 + 0.5)))
                    })
                    .collect::<Vec<_>>();
                assert_eq!(owners.len(), 1);
                assert_eq!(owners[0].style.density, if x < 40 { 1 } else { 2 });
            }
        }
    }

    #[test]
    #[ignore = "manual CPU paint-plan benchmark; excludes GPU upload and presentation"]
    fn shade_plan_vs_naive_pixels_benchmark() {
        use std::hint::black_box;
        use std::time::{Duration, Instant};

        use super::push_rect;

        let glyphs = (0..80)
            .map(|col| CellGlyph {
                col,
                ch: '▓',
                color: 0x123456,
            })
            .collect::<Vec<_>>();
        for scale in [1., 1.5, 2.] {
            let naive_pixels = || {
                let mut quads = Vec::new();
                for row in 0..24 {
                    for col in 0..80 {
                        let cell = cell_rect(col, col + 1, row, geometry(), scale);
                        let mut rects = Vec::new();
                        // Algorithmic control only: production previously used font
                        // glyphs, not this naive per-pixel rectangle builder.
                        for y in cell.top as i32..cell.bottom as i32 {
                            for x in cell.left as i32..cell.right as i32 {
                                if x.rem_euclid(2) == 1 && y.rem_euclid(2) == 0
                                    || x.rem_euclid(2) == y.rem_euclid(2)
                                {
                                    rects.push(Rect {
                                        left: x as f32,
                                        top: y as f32,
                                        right: x as f32 + 1.,
                                        bottom: y as f32 + 1.,
                                    });
                                }
                            }
                        }
                        for rect in rects {
                            push_rect(rect, 0x123456, scale, &mut quads);
                        }
                    }
                }
                black_box(&quads).len()
            };
            let optimized = || {
                let mut plan = CellGlyphPaintPlan::default();
                for row in 0..24 {
                    push_cell_glyphs(&glyphs, row, geometry(), scale, &mut plan);
                }
                plan.finish();
                black_box(&plan).primitive_count()
            };
            let measure = |iterations, build: &mut dyn FnMut() -> usize| {
                let started = Instant::now();
                let mut count = 0;
                for _ in 0..iterations {
                    count = build();
                }
                (started.elapsed() / iterations, count)
            };
            let (old_time, old_count) = measure(4, &mut || naive_pixels());
            let (new_time, new_count) = measure(256, &mut || optimized());
            assert!(new_count < old_count / 100);
            assert!(new_time > Duration::ZERO);
            println!(
                "scale={scale}: naive pixel quads {old_count} -> tiled sprites {new_count}; CPU plan {old_time:?} -> {new_time:?}"
            );
        }
    }

    #[test]
    fn solid_box_strokes_reach_cell_edges_and_keep_double_lines_separate() {
        let cell = Rect {
            left: 0.,
            top: 0.,
            right: 12.,
            bottom: 24.,
        };
        for (ch, expected) in [
            ('─', [1, 1, 0, 0]),
            ('┏', [0, 2, 0, 2]),
            ('╬', [3, 3, 3, 3]),
            ('┽', [2, 1, 1, 1]),
        ] {
            assert_eq!(box_arms(ch), Some(expected));
            let rects = box_rects(expected, cell, 1.);
            for (arm, weight) in expected.into_iter().enumerate() {
                if weight == 0 {
                    continue;
                }
                assert!(rects.iter().any(|r| match arm {
                    0 => r.left == cell.left,
                    1 => r.right == cell.right,
                    2 => r.top == cell.top,
                    _ => r.bottom == cell.bottom,
                }));
            }
        }
        let double = box_rects(box_arms('═').unwrap(), cell, 1.);
        assert!(double.iter().all(|r| !(r.top <= 12. && r.bottom > 12.)));
        for ch in ['A', '中', '⣿', '', '╭', '┄'] {
            assert!(!is_cell_glyph(ch));
        }
    }
}
