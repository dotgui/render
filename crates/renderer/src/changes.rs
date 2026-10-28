//! What an edit changed on a page, as the areas that need painting again.
//!
//! Two scenes of the same page — before an edit and after it — are walked
//! together. Where a node paints the same way in both, its children are
//! compared in turn; where it does not, everything it paints before and after
//! is marked, in canvas pixels, through whatever its ancestors do to it:
//! transforms move it, and a blur on an ancestor's layer spreads it. Children
//! are paired by tag and layer name in order, so a node inserted or removed
//! marks itself rather than every sibling after it.
//!
//! Repainting the marked areas of the after scene onto the before image gives
//! the after image. Painting outside them is untouched by the edit, with the
//! same one allowance region painting has: a shape crossing an area's edge can
//! differ along its anti-aliased outline.

use crate::paint::{
    backdrop_margin, layer_blur_sigma, node_matrix, paint_extent, scene_pixel_size, Extent,
    LAYER_FILTER_MARGIN,
};
use crate::{blur, LayoutRect, Scene, SceneNode};
use tiny_skia::Transform;

/// How much of the page an edit repainted.
#[derive(Debug, Clone, PartialEq)]
pub enum SceneChanges {
    /// Nothing paints differently.
    Nothing,
    /// Only these areas paint differently, in the scenes' pixels, snapped out
    /// to whole pixels and kept inside the page.
    Areas(Vec<LayoutRect>),
    /// The page changed size, or so much of it changed that painting it whole
    /// costs no more than painting the areas.
    Everything,
}

/// Areas past this many are merged into one: each is a separate paint.
const MAX_AREAS: usize = 16;

/// Past this share of the page, the page is painted whole.
const MOSTLY_CHANGED: f32 = 0.6;

/// The areas of the page that paint differently in `after` than in `before`.
/// Both scenes must be at the same scale.
pub fn scene_changes(before: &Scene, after: &Scene) -> SceneChanges {
    let size = scene_pixel_size(after);
    if scene_pixel_size(before) != size {
        return SceneChanges::Everything;
    }

    let mut marked = Vec::new();
    compare(&before.root, &after.root, Context::page(), &mut marked);
    if marked.is_empty() {
        return SceneChanges::Nothing;
    }

    let page = Extent {
        left: 0.0,
        top: 0.0,
        right: size.0 as f32,
        bottom: size.1 as f32,
    };
    // A backdrop blur reads what is around it, so a change near one changes
    // the blur a little further out.
    let spread = backdrop_margin(&before.root).max(backdrop_margin(&after.root)) as f32 + 1.0;
    let areas: Vec<Extent> = marked
        .into_iter()
        .map(|extent| snap(extent.grown(spread, spread).intersection(page)))
        .filter(|extent| area(*extent) > 0.0)
        .collect();
    let areas = merge(areas);

    let page_area = area(page);
    if areas.is_empty() {
        return SceneChanges::Nothing;
    }
    if page_area > 0.0 && areas.iter().copied().map(area).sum::<f32>() >= page_area * MOSTLY_CHANGED
    {
        return SceneChanges::Everything;
    }
    SceneChanges::Areas(
        areas
            .into_iter()
            .map(|extent| LayoutRect {
                x: extent.left,
                y: extent.top,
                width: extent.right - extent.left,
                height: extent.bottom - extent.top,
            })
            .collect(),
    )
}

/// Where a subtree's pixels land: the transforms of the nodes above it, and
/// how far the blurs on their layers spread it.
#[derive(Clone, Copy)]
struct Context {
    transform: Transform,
    spread: f32,
}

impl Context {
    fn page() -> Self {
        Self {
            transform: Transform::identity(),
            spread: 0.0,
        }
    }

    /// The context `node`'s children paint in.
    fn inside(self, node: &SceneNode) -> Self {
        let transform = match node.transform.filter(|transform| !transform.is_identity()) {
            Some(transform) => self.transform.pre_concat(node_matrix(node, transform)),
            None => self.transform,
        };
        Self {
            transform,
            spread: self.spread + blur::reach(layer_blur_sigma(node)) as f32,
        }
    }

    /// Everything `node` paints, including its own transform, placed on the
    /// canvas.
    fn place(self, node: &SceneNode) -> Extent {
        let mut extent = paint_extent(node);
        if let Some(transform) = node.transform.filter(|transform| !transform.is_identity()) {
            extent = extent
                .grown(LAYER_FILTER_MARGIN, LAYER_FILTER_MARGIN)
                .transformed(node_matrix(node, transform));
        }
        if !self.transform.is_identity() {
            extent = extent
                .grown(LAYER_FILTER_MARGIN, LAYER_FILTER_MARGIN)
                .transformed(self.transform);
        }
        extent.grown(self.spread, self.spread)
    }
}

fn compare(before: &SceneNode, after: &SceneNode, context: Context, marked: &mut Vec<Extent>) {
    if !paints_the_same_itself(before, after) {
        marked.push(context.place(before));
        marked.push(context.place(after));
        return;
    }

    let inside = context.inside(after);
    let old: Vec<&SceneNode> = before.children.iter().filter(|child| !child.mask).collect();
    let new: Vec<&SceneNode> = after.children.iter().filter(|child| !child.mask).collect();
    let pairs = pair_children(&old, &new);

    let (mut next_old, mut next_new) = (0, 0);
    for (paired_old, paired_new) in pairs
        .iter()
        .copied()
        .chain(std::iter::once((old.len(), new.len())))
    {
        // What sits between two pairs was removed or inserted.
        for removed in &old[next_old..paired_old] {
            marked.push(inside.place(removed));
        }
        for inserted in &new[next_new..paired_new] {
            marked.push(inside.place(inserted));
        }
        if paired_old < old.len() {
            compare(old[paired_old], new[paired_new], inside, marked);
        }
        next_old = paired_old + 1;
        next_new = paired_new + 1;
    }
}

/// The longest run of children, in order, whose tag and layer name agree,
/// as index pairs.
fn pair_children(old: &[&SceneNode], new: &[&SceneNode]) -> Vec<(usize, usize)> {
    let same = |a: &SceneNode, b: &SceneNode| a.tag == b.tag && a.name == b.name;

    // The common case — an edit that adds or removes nothing — needs no table.
    if old.len() == new.len() && old.iter().zip(new).all(|(a, b)| same(a, b)) {
        return (0..old.len()).map(|index| (index, index)).collect();
    }

    let (rows, columns) = (old.len(), new.len());
    let mut lengths = vec![0u32; (rows + 1) * (columns + 1)];
    let at = |row: usize, column: usize| row * (columns + 1) + column;
    for row in (0..rows).rev() {
        for column in (0..columns).rev() {
            lengths[at(row, column)] = if same(old[row], new[column]) {
                lengths[at(row + 1, column + 1)] + 1
            } else {
                lengths[at(row + 1, column)].max(lengths[at(row, column + 1)])
            };
        }
    }

    let mut pairs = Vec::new();
    let (mut row, mut column) = (0, 0);
    while row < rows && column < columns {
        if same(old[row], new[column]) {
            pairs.push((row, column));
            row += 1;
            column += 1;
        } else if lengths[at(row + 1, column)] >= lengths[at(row, column + 1)] {
            row += 1;
        } else {
            column += 1;
        }
    }
    pairs
}

/// Whether two nodes paint the same themselves, leaving their painted
/// children aside. A masking child shapes its parent, so it counts as part of
/// the parent.
///
/// Every field is named, so a field added to [`SceneNode`] does not compile
/// here until it is decided whether it changes what a node paints.
fn paints_the_same_itself(before: &SceneNode, after: &SceneNode) -> bool {
    let SceneNode {
        tag,
        name: _,
        bounds,
        fills,
        borders,
        outline,
        radius,
        corner_smoothing,
        blend,
        isolation,
        filter,
        mask,
        image_mask,
        clip_path,
        transform,
        href: _,
        opacity,
        visible,
        clip_x,
        clip_y,
        effects,
        content,
        children,
    } = before;

    let masks = |children: &[SceneNode]| {
        children
            .iter()
            .filter(|child| child.mask)
            .cloned()
            .collect::<Vec<_>>()
    };

    *tag == after.tag
        && *bounds == after.bounds
        && *fills == after.fills
        && *borders == after.borders
        && *outline == after.outline
        && *radius == after.radius
        && *corner_smoothing == after.corner_smoothing
        && *blend == after.blend
        && *isolation == after.isolation
        && *filter == after.filter
        && *mask == after.mask
        && *image_mask == after.image_mask
        && *clip_path == after.clip_path
        && *transform == after.transform
        && *opacity == after.opacity
        && *visible == after.visible
        && *clip_x == after.clip_x
        && *clip_y == after.clip_y
        && *effects == after.effects
        && *content == after.content
        && masks(children) == masks(&after.children)
}

fn snap(extent: Extent) -> Extent {
    Extent {
        left: extent.left.floor(),
        top: extent.top.floor(),
        right: extent.right.ceil(),
        bottom: extent.bottom.ceil(),
    }
}

fn area(extent: Extent) -> f32 {
    (extent.right - extent.left).max(0.0) * (extent.bottom - extent.top).max(0.0)
}

/// Merges areas that overlap, or that cost no more to paint as one; then, past
/// [`MAX_AREAS`], merges everything into one.
fn merge(mut areas: Vec<Extent>) -> Vec<Extent> {
    let mut merged = true;
    while merged {
        merged = false;
        'search: for first in 0..areas.len() {
            for second in first + 1..areas.len() {
                let (a, b) = (areas[first], areas[second]);
                let union = a.union(b);
                if a.overlaps(b) || area(union) <= (area(a) + area(b)) * 1.25 {
                    areas[first] = union;
                    areas.swap_remove(second);
                    merged = true;
                    break 'search;
                }
            }
        }
    }
    if areas.len() > MAX_AREAS {
        let all = areas
            .iter()
            .copied()
            .reduce(Extent::union)
            .expect("more than one area");
        return vec![all];
    }
    areas
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{build_scene, compute_taffy_layout, parse_gui_xml};

    fn scene(xml: &str) -> Scene {
        let document = parse_gui_xml(xml).unwrap();
        let layout = compute_taffy_layout(&document).unwrap();
        build_scene(&document, &layout)
    }

    fn page(body: &str) -> String {
        format!(
            r##"<gui version="0.2"><frame w="400" h="300" fill="#ffffff">{body}</frame></gui>"##
        )
    }

    fn areas(before: &str, after: &str) -> SceneChanges {
        scene_changes(&scene(&page(before)), &scene(&page(after)))
    }

    #[test]
    fn nothing_changed_is_nothing() {
        let body = r##"<rect abs x="10" y="10" w="40" h="40" fill="#ff0000" />"##;
        assert_eq!(areas(body, body), SceneChanges::Nothing);
    }

    #[test]
    fn a_recoloured_node_marks_only_itself() {
        let before = r##"<rect abs x="10" y="10" w="40" h="40" fill="#ff0000" />
                         <rect abs x="300" y="200" w="40" h="40" fill="#00ff00" />"##;
        let after = r##"<rect abs x="10" y="10" w="40" h="40" fill="#0000ff" />
                        <rect abs x="300" y="200" w="40" h="40" fill="#00ff00" />"##;
        let SceneChanges::Areas(areas) = areas(before, after) else {
            panic!("expected areas");
        };
        assert_eq!(areas.len(), 1);
        let area = areas[0];
        assert!(area.x <= 10.0 && area.y <= 10.0);
        assert!(area.x + area.width >= 50.0 && area.y + area.height >= 50.0);
        assert!(area.width < 60.0 && area.height < 60.0, "{area:?}");
    }

    #[test]
    fn a_moved_node_marks_where_it_was_and_where_it_is() {
        let SceneChanges::Areas(areas) = areas(
            r##"<rect abs x="10" y="10" w="20" h="20" fill="#ff0000" />"##,
            r##"<rect abs x="300" y="250" w="20" h="20" fill="#ff0000" />"##,
        ) else {
            panic!("expected areas");
        };
        assert_eq!(areas.len(), 2, "two small areas far apart stay apart");
    }

    #[test]
    fn an_inserted_sibling_does_not_mark_the_siblings_after_it() {
        let SceneChanges::Areas(areas) = areas(
            r##"<rect abs x="10" y="10" w="20" h="20" fill="#ff0000" />
                <rect abs x="300" y="250" w="20" h="20" fill="#00ff00" />"##,
            r##"<ellipse abs x="150" y="120" w="20" h="20" fill="#0000ff" />
                <rect abs x="10" y="10" w="20" h="20" fill="#ff0000" />
                <rect abs x="300" y="250" w="20" h="20" fill="#00ff00" />"##,
        ) else {
            panic!("expected areas");
        };
        assert_eq!(areas.len(), 1);
        assert!(areas[0].x > 100.0 && areas[0].y > 100.0, "{:?}", areas[0]);
    }

    #[test]
    fn a_change_inside_a_rotated_group_is_marked_where_it_lands() {
        let before = r##"<group abs x="100" y="100" w="100" h="20" rotation="90">
                           <rect w="100" h="20" fill="#ff0000" />
                         </group>"##;
        let after = before.replace("#ff0000", "#0000ff");
        let SceneChanges::Areas(areas) = areas(before, &after) else {
            panic!("expected areas");
        };
        // Turned a quarter, the 100×20 group stands 100 tall.
        assert!(areas[0].height >= 100.0, "{:?}", areas[0]);
    }

    #[test]
    fn a_page_that_changes_size_or_mostly_changes_is_everything() {
        let before = scene(&page(""));
        let taller =
            scene(r##"<gui version="0.2"><frame w="400" h="500" fill="#ffffff" /></gui>"##);
        assert_eq!(scene_changes(&before, &taller), SceneChanges::Everything);

        let recoloured =
            scene(r##"<gui version="0.2"><frame w="400" h="300" fill="#000000" /></gui>"##);
        assert_eq!(
            scene_changes(&before, &recoloured),
            SceneChanges::Everything
        );
    }

    #[test]
    fn a_change_under_a_blurred_layer_is_marked_as_far_as_the_blur_spreads_it() {
        let before = r##"<group abs x="100" y="100" w="100" h="100">
                           <appearance><effect type="layer-blur" radius="10" /></appearance>
                           <rect w="20" h="20" fill="#ff0000" />
                         </group>"##;
        let after = before.replace("#ff0000", "#0000ff");
        let SceneChanges::Areas(areas) = areas(before, &after) else {
            panic!("expected areas");
        };
        let reach = blur::reach(10.0) as f32;
        // The rect sits at 100,100; blurred, its colour reaches `reach` further.
        assert!(areas[0].x <= 100.0 - reach, "{:?}, reach {reach}", areas[0]);
        assert!(
            areas[0].x + areas[0].width >= 120.0 + reach,
            "{:?}",
            areas[0]
        );
    }
}
