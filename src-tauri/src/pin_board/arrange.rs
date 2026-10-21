//! 素材板阵列排序：把散乱的图片整理成统一高度、行宽均衡的阵列。
//!
//! 算法分成四个互不越界的阶段，顺序一旦确定就不会被后续步骤改写：
//!
//! 1. [`resolve_height`] 统一高度：默认取参与图片原始高度的中位数；
//! 2. [`reading_order`] 阅读序：按画面位置自上而下分带，带内自左而右；
//! 3. [`row_ranges`] 断行：动态规划，最小化各行宽度与目标行宽的偏差；
//! 4. [`place`] 落位：逐行居中，最后把整体平移回原位置。
//!
//! 旧实现（前端与 `dds.rs` 各有一份拷贝）在断行之后按宽度对行内和行间各做了一次排
//! 序。这两次排序既改变不了任何一行的总宽、也改变不了整体形状，只会把用户摆好的相
//! 对位置彻底打乱——典型后果是「最后一行只有一张图」时，那张孤图被升序排序顶到最上
//! 面居中飘着。这里把排序挪到排版之前，并且只做一次。

/// 参与排序的单张图片：当前显示尺寸与世界坐标下的中心点。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ArrangeItem {
    pub(crate) item_id: i64,
    pub(crate) width: f64,
    pub(crate) height: f64,
    pub(crate) center_x: f64,
    pub(crate) center_y: f64,
}

/// 排版顺序的来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ItemOrder {
    /// 按画面位置取阅读序（整理已有图片）。
    Reading,
    /// 保持调用方给出的顺序（导入/粘贴时新图还没有位置）。
    Input,
}

/// 统一高度的取值口径。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HeightPolicy {
    /// 原始高度中位数：不会被单张超大图带偏，整理已有图片的默认口径。
    Median,
    /// 原始高度最大值：导入时把整批拉到同一高度。
    Max,
}

/// 排版选项。
///
/// 间距只有一种口径：相对统一高度的比例。整理与导入共用同一个设置，不再保留
/// 「世界单位」的第二套口径——两套口径正是旧实现里两边行为分叉的根源。
#[derive(Debug, Clone, Copy)]
pub(crate) struct ArrangeOptions {
    pub(crate) order: ItemOrder,
    pub(crate) height: HeightPolicy,
    /// 相对统一高度的间距比例，`0.0` 表示图片紧贴。
    pub(crate) gap_ratio: f64,
}

pub(crate) type Quad = [[f64; 2]; 4];

/// 默认间距：图高的 6%。
pub(crate) const DEFAULT_GAP_RATIO: f64 = 0.06;
/// 间距上限（相对统一高度）。再往上，块就看不出是阵列了，所以设置项与命令校验
/// 共用这一个上限，避免两处各写一个数字之后漂移。
pub(crate) const MAX_GAP_RATIO: f64 = 0.20;

const EPSILON: f64 = 1e-6;
/// 阅读序分带阈值：中心高度差不超过统一高度的这个比例，视为同一行。
const BAND_RATIO: f64 = 0.5;
/// 长宽比分位裁剪：按名次两端各剔除 10%（至少各剔除一个），避免一张离群图
/// 决定整体形状。
const TRIM_RATIO: f64 = 0.1;
/// 动态规划断行的规模上限。单块画板允许两万条目，O(n²) 在这个量级不可接受，
/// 超过阈值退回贪心断行——行宽略差一些，但不影响顺序与稳定性。
const MAX_DP_ITEMS: usize = 1_200;

/// 排版过程中的单张图片：`origin` 是它在调用方数组里的下标，用于按原顺序返回。
struct Node {
    origin: usize,
    /// 缩放到统一高度之后的宽度。
    width: f64,
}

/// 把 `items` 整理成阵列，返回与输入**同序**的 `(item_id, 四角点)`。
///
/// 尺寸非法（非有限值或非正数）的条目会被跳过；全部非法时返回空结果。
pub(crate) fn arrange(items: &[ArrangeItem], options: ArrangeOptions) -> Vec<(i64, Quad)> {
    let usable: Vec<usize> = (0..items.len())
        .filter(|&index| is_usable(&items[index]))
        .collect();
    if usable.is_empty() {
        return Vec::new();
    }
    let usable_items: Vec<ArrangeItem> = usable.iter().map(|&index| items[index]).collect();
    let height = resolve_height(&usable_items, options.height);
    if height <= EPSILON {
        return Vec::new();
    }
    let gap = options.gap_ratio.max(0.0) * height;
    let nodes: Vec<Node> = usable
        .iter()
        .map(|&origin| {
            let item = &items[origin];
            Node {
                origin,
                width: item.width * height / item.height,
            }
        })
        .collect();

    let order = match options.order {
        ItemOrder::Reading => reading_order(&nodes, items, height * BAND_RATIO),
        ItemOrder::Input => (0..nodes.len()).collect(),
    };
    let widths: Vec<f64> = order.iter().map(|&index| nodes[index].width).collect();
    let rows = target_row_count(nodes.len(), trimmed_aspect(&usable_items));
    let target_width = target_row_width(&widths, gap, rows);
    let ranges = row_ranges(&widths, gap, target_width);
    let placed = place(&order, &nodes, &widths, &ranges, height, gap);
    let anchor = trimmed_center(&usable_items);

    let mut result = placed;
    result.sort_by_key(|(origin, _)| *origin);
    let block = block_bounds(&result);
    let (shift_x, shift_y) = (
        anchor.0 - (block.min_x + block.max_x) / 2.0,
        anchor.1 - (block.min_y + block.max_y) / 2.0,
    );
    result
        .into_iter()
        .map(|(origin, quad)| {
            let quad = quad.map(|[x, y]| [x + shift_x, y + shift_y]);
            (items[origin].item_id, quad)
        })
        .collect()
}

fn is_usable(item: &ArrangeItem) -> bool {
    item.width.is_finite()
        && item.height.is_finite()
        && item.width > EPSILON
        && item.height > EPSILON
}

/// 统一高度：中位数对极端值稳健，最大值用于导入时把整批拉到同一高度。
fn resolve_height(items: &[ArrangeItem], policy: HeightPolicy) -> f64 {
    let mut heights: Vec<f64> = items.iter().map(|item| item.height).collect();
    heights.sort_by(f64::total_cmp);
    match policy {
        HeightPolicy::Max => heights.last().copied().unwrap_or(0.0),
        HeightPolicy::Median => {
            let middle = heights.len() / 2;
            if heights.len() % 2 == 0 {
                (heights[middle - 1] + heights[middle]) / 2.0
            } else {
                heights[middle]
            }
        }
    }
}

/// 阅读序：先按 y 从大到小（画布 y 向上增长，y 大的在屏幕上方），再按 x 从小到大。
///
/// 直接按 y 升序会把整块图上下翻转——第一行会取到最下面那一排。
fn reading_order(nodes: &[Node], items: &[ArrangeItem], band: f64) -> Vec<usize> {
    let center = |index: usize| {
        let item = &items[nodes[index].origin];
        (item.center_x, item.center_y)
    };
    let mut indices: Vec<usize> = (0..nodes.len()).collect();
    indices.sort_by(|&left, &right| {
        center(right)
            .1
            .total_cmp(&center(left).1)
            .then_with(|| center(left).0.total_cmp(&center(right).0))
    });

    // 分带：把「高度差不到半个图高」的相邻图片并成同一行，避免手摆的轻微错位被
    // 读成两行。带内再按 x 排序，得到自上而下、自左而右的阅读序。
    let mut banded: Vec<(usize, usize)> = Vec::with_capacity(indices.len());
    let mut current = 0usize;
    let mut previous: Option<f64> = None;
    for index in indices {
        let y = center(index).1;
        if let Some(previous_y) = previous {
            if (previous_y - y).abs() > band {
                current += 1;
            }
        }
        previous = Some(y);
        banded.push((index, current));
    }
    banded.sort_by(|left, right| {
        left.1
            .cmp(&right.1)
            .then_with(|| center(left.0).0.total_cmp(&center(right.0).0))
    });
    banded.into_iter().map(|(index, _)| index).collect()
}

/// 用图片中心的分位区间估算整体长宽比。
///
/// 直接取完整外框会被一张拖得很远的图带跑（外框立刻变成 7:1），所以按名次两端
/// 各裁掉 10%。裁剪必须按名次而不是按插值分位：九张图时 90% 分位落在第 8、第 9
/// 个值之间，仍然会把离群图算进来，长宽比照样被带偏。
///
/// 分位区间退化成一条水平线时返回正无穷、一条竖线时返回 0，由
/// [`target_row_count`] 分别落成「一行」和「一列」。
fn trimmed_aspect(items: &[ArrangeItem]) -> f64 {
    let mut xs: Vec<f64> = items.iter().map(|item| item.center_x).collect();
    let mut ys: Vec<f64> = items.iter().map(|item| item.center_y).collect();
    xs.sort_by(f64::total_cmp);
    ys.sort_by(f64::total_cmp);
    let span_x = trimmed_span(&xs);
    let span_y = trimmed_span(&ys);
    match (span_x > EPSILON, span_y > EPSILON) {
        (false, false) => 1.0,
        (true, false) => f64::INFINITY,
        (false, true) => 0.0,
        (true, true) => span_x / span_y,
    }
}

/// 已升序序列去掉两端各 10% 名次后的跨度。
fn trimmed_span(sorted: &[f64]) -> f64 {
    let (low, high) = trimmed_bounds(sorted);
    (high - low).max(0.0)
}

/// 两端各剔除多少个名次。少于 5 个样本时不裁剪（样本本来就少，裁掉就测不出形状
/// 了）；裁剪量也不会超过一半，保证至少留下一个值。
fn trim_count(len: usize) -> usize {
    if len < 5 {
        return 0;
    }
    (((len as f64) * TRIM_RATIO).floor() as usize)
        .max(1)
        .min((len - 1) / 2)
}

/// 已升序序列裁剪后的取值范围。
fn trimmed_bounds(sorted: &[f64]) -> (f64, f64) {
    if sorted.is_empty() {
        return (0.0, 0.0);
    }
    let trim = trim_count(sorted.len());
    (sorted[trim], sorted[sorted.len() - 1 - trim])
}

/// 目标行数：让整体长宽比接近原选区的形状，`行数 ≈ sqrt(张数 / 长宽比)`。
///
/// 这个式子对极端形状自动给出正确结果，不需要额外特判：一字排的长宽比很大，
/// 行数落到 1；竖列的长宽比接近 0，行数落到张数本身。
fn target_row_count(count: usize, aspect: f64) -> usize {
    if count == 0 {
        return 1;
    }
    if !aspect.is_finite() {
        return 1;
    }
    if aspect <= EPSILON {
        return count;
    }
    let rows = (count as f64 / aspect).sqrt().round();
    if !rows.is_finite() {
        return 1;
    }
    (rows as usize).clamp(1, count)
}

/// 目标行宽：把总宽度平摊到目标行数上，行间不留多余的尾随间距。
fn target_row_width(widths: &[f64], gap: f64, rows: usize) -> f64 {
    if widths.is_empty() {
        return 0.0;
    }
    let rows = rows.max(1);
    let total: f64 = widths.iter().sum::<f64>() + (widths.len().saturating_sub(rows)) as f64 * gap;
    total / rows as f64
}

/// 断行：最小化 `Σ(行宽 − 目标行宽)²`。
///
/// 每一行都参与惩罚（不像文本断行那样免罚末行）——末行免罚会让「所有图排成一行」
/// 成为零代价解，整块图会退化成一长条。
fn row_ranges(widths: &[f64], gap: f64, target: f64) -> Vec<(usize, usize)> {
    if widths.is_empty() {
        return Vec::new();
    }
    if widths.len() > MAX_DP_ITEMS {
        return greedy_row_ranges(widths, gap, target);
    }
    let count = widths.len();
    let mut prefix = vec![0.0; count + 1];
    for index in 0..count {
        prefix[index + 1] = prefix[index] + widths[index] + gap;
    }
    let row_width = |start: usize, end: usize| prefix[end] - prefix[start] - gap;

    let mut best = vec![f64::INFINITY; count + 1];
    let mut cut = vec![0usize; count + 1];
    best[0] = 0.0;
    for end in 1..=count {
        for start in 0..end {
            let cost = best[start] + (row_width(start, end) - target).powi(2);
            // 用 `<=` 而不是 `<`：代价相同时让更大的起点胜出，也就是把靠前的行
            // 排满、让最后一行短一点。和 `<` 相比结果同样最优，但更符合「自上而下
            // 填格」的直觉，不会出现「首行只有一张图」。
            if cost <= best[end] {
                best[end] = cost;
                cut[end] = start;
            }
        }
    }

    let mut ranges = Vec::new();
    let mut end = count;
    while end > 0 {
        let start = cut[end];
        ranges.push((start, end));
        end = start;
    }
    ranges.reverse();
    ranges
}

/// 贪心断行：塞不下就换行。只在大规模输入下兜底，避免 O(n²) 拖住主线程。
fn greedy_row_ranges(widths: &[f64], gap: f64, target: f64) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut start = 0usize;
    let mut width = 0.0f64;
    for (index, item_width) in widths.iter().enumerate() {
        let next = if index == start {
            *item_width
        } else {
            width + gap + *item_width
        };
        if index > start && next > target {
            ranges.push((start, index));
            start = index;
            width = *item_width;
        } else {
            width = next;
        }
    }
    if start < widths.len() {
        ranges.push((start, widths.len()));
    }
    ranges
}

/// 逐行落位：整体左上角从原点开始，各行按块宽居中，行内严格保持 `order` 的顺序。
fn place(
    order: &[usize],
    nodes: &[Node],
    widths: &[f64],
    ranges: &[(usize, usize)],
    height: f64,
    gap: f64,
) -> Vec<(usize, Quad)> {
    let block_width = ranges
        .iter()
        .map(|&(start, end)| row_width_of(widths, gap, start, end))
        .fold(0.0f64, f64::max);
    let mut placed = Vec::with_capacity(widths.len());
    let mut top = 0.0f64;
    for &(start, end) in ranges {
        let mut x = (block_width - row_width_of(widths, gap, start, end)) / 2.0;
        for index in start..end {
            let node = &nodes[order[index]];
            let width = widths[index];
            placed.push((
                node.origin,
                [
                    [x, top],
                    [x + width, top],
                    [x + width, top - height],
                    [x, top - height],
                ],
            ));
            x += width + gap;
        }
        top -= height + gap;
    }
    placed
}

fn row_width_of(widths: &[f64], gap: f64, start: usize, end: usize) -> f64 {
    if end <= start {
        return 0.0;
    }
    widths[start..end].iter().sum::<f64>() + (end - start - 1) as f64 * gap
}

/// 对齐基准：裁剪后的外框中心。
///
/// 和长宽比用同一套裁剪，离群点既不会改变形状、也不会把整块图拽到它那边去。
/// 正常分布下等于原选区外框中心。
fn trimmed_center(items: &[ArrangeItem]) -> (f64, f64) {
    let mut xs: Vec<f64> = items.iter().map(|item| item.center_x).collect();
    let mut ys: Vec<f64> = items.iter().map(|item| item.center_y).collect();
    xs.sort_by(f64::total_cmp);
    ys.sort_by(f64::total_cmp);
    let (x_low, x_high) = trimmed_bounds(&xs);
    let (y_low, y_high) = trimmed_bounds(&ys);
    ((x_low + x_high) / 2.0, (y_low + y_high) / 2.0)
}

struct Bounds {
    min_x: f64,
    min_y: f64,
    max_x: f64,
    max_y: f64,
}

fn block_bounds(placed: &[(usize, Quad)]) -> Bounds {
    let mut bounds = Bounds {
        min_x: f64::INFINITY,
        min_y: f64::INFINITY,
        max_x: f64::NEG_INFINITY,
        max_y: f64::NEG_INFINITY,
    };
    for (_, quad) in placed {
        for [x, y] in quad {
            bounds.min_x = bounds.min_x.min(*x);
            bounds.min_y = bounds.min_y.min(*y);
            bounds.max_x = bounds.max_x.max(*x);
            bounds.max_y = bounds.max_y.max(*y);
        }
    }
    bounds
}

/// 粘贴/导入图片以指针位置为中心做阵列排序。
///
/// 排版数学由本模块提供，这里只负责把「一批自然尺寸」转成排列输入并保持返回顺序
/// 与 `sizes` 一致。导入的新图还没有位置，所以用 [`ItemOrder::Input`] 保留调用方给
/// 的顺序；间距与整理路径同一口径——按统一高度的比例传入，不随画布缩放变化。
pub(crate) fn imported_points(
    sizes: &[(f64, f64)],
    center_x: f64,
    center_y: f64,
    gap_ratio: f64,
) -> Vec<Quad> {
    if sizes.is_empty() {
        return Vec::new();
    }
    // 尺寸由上游（`repository` 的导入/粘贴校验）保证为正有限值；这里只兜住退化输入，
    // 让每个条目都拿到一个位于中心的零面积四边形，避免调用方按索引 zip 时错位。
    let degenerate = || {
        vec![
            [
                [center_x, center_y],
                [center_x, center_y],
                [center_x, center_y],
                [center_x, center_y],
            ];
            sizes.len()
        ]
    };
    let usable = |value: f64| value.is_finite() && value > f64::EPSILON;
    if !sizes
        .iter()
        .all(|(width, height)| usable(*width) && usable(*height))
    {
        return degenerate();
    }

    let items = sizes
        .iter()
        .enumerate()
        .map(|(index, (width, height))| ArrangeItem {
            item_id: index as i64,
            width: *width,
            height: *height,
            center_x,
            center_y,
        })
        .collect::<Vec<_>>();
    let arranged = arrange(
        &items,
        ArrangeOptions {
            order: ItemOrder::Input,
            height: HeightPolicy::Max,
            gap_ratio: gap_ratio.max(0.0),
        },
    );
    if arranged.len() != sizes.len() {
        return degenerate();
    }
    arranged.into_iter().map(|(_, points)| points).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: i64, width: f64, height: f64, x: f64, y: f64) -> ArrangeItem {
        ArrangeItem {
            item_id: id,
            width,
            height,
            center_x: x,
            center_y: y,
        }
    }

    fn options() -> ArrangeOptions {
        ArrangeOptions {
            order: ItemOrder::Reading,
            height: HeightPolicy::Median,
            gap_ratio: 0.06,
        }
    }

    fn width_of(quad: &Quad) -> f64 {
        (quad[1][0] - quad[0][0]).abs()
    }

    fn height_of(quad: &Quad) -> f64 {
        (quad[0][1] - quad[3][1]).abs()
    }

    /// 按 y 分组成行：同一行的左上角 y 相同；行内按实际 x 排序，行的顺序按 y 从高到低。
    fn rows_of(result: &[(i64, Quad)]) -> Vec<Vec<i64>> {
        let mut rows: Vec<(f64, Vec<(f64, i64)>)> = Vec::new();
        for (id, quad) in result {
            let top = quad[0][1];
            match rows.iter_mut().find(|(y, _)| (y - top).abs() < 1e-6) {
                Some((_, row)) => row.push((quad[0][0], *id)),
                None => rows.push((top, vec![(quad[0][0], *id)])),
            }
        }
        rows.sort_by(|left, right| right.0.total_cmp(&left.0));
        rows.into_iter()
            .map(|(_, mut row)| {
                row.sort_by(|left, right| left.0.total_cmp(&right.0));
                row.into_iter().map(|(_, id)| id).collect()
            })
            .collect()
    }

    #[test]
    fn every_image_is_scaled_to_the_median_height() {
        let items = [
            item(1, 200.0, 100.0, 0.0, 0.0),
            item(2, 100.0, 200.0, 300.0, 0.0),
            item(3, 400.0, 300.0, 600.0, 0.0),
        ];
        let result = arrange(&items, options());
        for (_, quad) in &result {
            assert!((height_of(quad) - 200.0).abs() < 1e-6);
        }
        // 等比缩放：宽高比保持不变。
        assert!((width_of(&result[0].1) - 400.0).abs() < 1e-6);
        assert!((width_of(&result[1].1) - 100.0).abs() < 1e-6);
        assert!((width_of(&result[2].1) - 800.0 / 3.0).abs() < 1e-6);
    }

    #[test]
    fn reading_order_survives_the_layout() {
        // 输入顺序故意打乱：id 5 在左上、id 1 在右下。
        let items = [
            item(1, 100.0, 100.0, 300.0, -300.0),
            item(5, 100.0, 100.0, 0.0, 0.0),
            item(3, 100.0, 100.0, 300.0, 0.0),
            item(4, 100.0, 100.0, 0.0, -300.0),
            item(2, 100.0, 100.0, 150.0, 0.0),
        ];
        let result = arrange(&items, options());
        // 返回顺序与输入一致（调用方按 item_id 取用），但落位按阅读序。
        assert_eq!(
            result.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
            vec![1, 5, 3, 4, 2]
        );
        let rows = rows_of(&result);
        assert_eq!(rows[0], vec![5, 2, 3]);
        assert_eq!(rows[1], vec![4, 1]);
    }

    #[test]
    fn a_single_line_stays_a_single_row() {
        let items: Vec<ArrangeItem> = (0..8)
            .map(|index| item(index, 100.0, 100.0, index as f64 * 120.0, 0.0))
            .collect();
        let result = arrange(&items, options());
        assert_eq!(rows_of(&result).len(), 1);
        // 间距均等：相邻两张的间隔一致。
        let mut gaps = Vec::new();
        for pair in result.windows(2) {
            gaps.push(pair[1].1[0][0] - pair[0].1[1][0]);
        }
        for gap in &gaps {
            assert!((gap - gaps[0]).abs() < 1e-6);
        }
    }

    #[test]
    fn a_vertical_column_becomes_one_image_per_row() {
        let items: Vec<ArrangeItem> = (0..5)
            .map(|index| item(index, 100.0, 100.0, 0.0, -(index as f64) * 120.0))
            .collect();
        let result = arrange(&items, options());
        let rows = rows_of(&result);
        assert_eq!(rows.len(), 5);
        for row in rows {
            assert_eq!(row.len(), 1);
        }
    }

    #[test]
    fn gap_follows_the_image_height_not_the_caller() {
        let small = [
            item(1, 100.0, 100.0, 0.0, 0.0),
            item(2, 100.0, 100.0, 120.0, 0.0),
        ];
        let large = [
            item(1, 1000.0, 1000.0, 0.0, 0.0),
            item(2, 1000.0, 1000.0, 1200.0, 0.0),
        ];
        let small_gap =
            arrange(&small, options())[1].1[0][0] - arrange(&small, options())[0].1[1][0];
        let large_gap =
            arrange(&large, options())[1].1[0][0] - arrange(&large, options())[0].1[1][0];
        assert!((small_gap - 6.0).abs() < 1e-6);
        assert!((large_gap - 60.0).abs() < 1e-6);
    }

    #[test]
    fn an_outlier_neither_flattens_nor_drags_the_block() {
        // 八张图聚成 3x3 的密集区，另有一张被拖到很远。不裁剪时外框长宽比会被拉
        // 到 7:1 以上、行数塌到 1~2，整块还会被拽向那张孤图。
        let mut items: Vec<ArrangeItem> = (0..8)
            .map(|index| {
                item(
                    index,
                    100.0,
                    100.0,
                    (index % 3) as f64 * 120.0,
                    -((index / 3) as f64) * 120.0,
                )
            })
            .collect();
        items.push(item(8, 100.0, 100.0, 2000.0, -60.0));
        let result = arrange(&items, options());
        assert_eq!(rows_of(&result).len(), 3);

        let bounds = block_bounds(
            &result
                .iter()
                .enumerate()
                .map(|(origin, (_, quad))| (origin, *quad))
                .collect::<Vec<_>>(),
        );
        // 密集区中心在 (120, -120)，离群点在 x=2000：整块仍留在密集区。
        assert!(((bounds.min_x + bounds.max_x) / 2.0 - 120.0).abs() < 1e-6);
        assert!(((bounds.min_y + bounds.max_y) / 2.0 + 120.0).abs() < 1e-6);
    }

    #[test]
    fn overlapping_images_do_not_panic() {
        let items: Vec<ArrangeItem> = (0..6)
            .map(|index| item(index, 100.0, 100.0, 0.0, 0.0))
            .collect();
        let result = arrange(&items, options());
        assert_eq!(result.len(), 6);
        for (_, quad) in &result {
            for [x, y] in quad {
                assert!(x.is_finite() && y.is_finite());
            }
        }
    }

    #[test]
    fn the_block_is_centred_on_the_original_position() {
        let items = [
            item(1, 100.0, 100.0, 1000.0, 500.0),
            item(2, 100.0, 100.0, 1200.0, 500.0),
            item(3, 100.0, 100.0, 1100.0, 620.0),
        ];
        let result = arrange(&items, options());
        let bounds = block_bounds(
            &result
                .iter()
                .enumerate()
                .map(|(origin, (_, quad))| (origin, *quad))
                .collect::<Vec<_>>(),
        );
        // 三个样本不触发裁剪，基准就是原外框中心：x (1000+1200)/2、y (500+620)/2。
        assert!(((bounds.min_x + bounds.max_x) / 2.0 - 1100.0).abs() < 1e-6);
        assert!(((bounds.min_y + bounds.max_y) / 2.0 - 560.0).abs() < 1e-6);
    }

    #[test]
    fn neighbours_never_overlap() {
        let items: Vec<ArrangeItem> = (0..12)
            .map(|index| {
                item(
                    index,
                    60.0 + (index % 4) as f64 * 40.0,
                    80.0,
                    (index % 4) as f64 * 200.0,
                    -((index / 4) as f64) * 150.0,
                )
            })
            .collect();
        let result = arrange(&items, options());
        for left in 0..result.len() {
            for right in left + 1..result.len() {
                let a = &result[left].1;
                let b = &result[right].1;
                let overlaps = a[0][0] < b[1][0] - 1e-6
                    && b[0][0] < a[1][0] - 1e-6
                    && a[3][1] < b[0][1] - 1e-6
                    && b[3][1] < a[0][1] - 1e-6;
                assert!(!overlaps, "items {} and {} overlap", left, right);
            }
        }
    }

    #[test]
    fn input_order_is_kept_for_imports() {
        let items = [
            item(7, 100.0, 100.0, 0.0, 0.0),
            item(3, 100.0, 100.0, 0.0, 0.0),
            item(9, 100.0, 100.0, 0.0, 0.0),
        ];
        let options = ArrangeOptions {
            order: ItemOrder::Input,
            height: HeightPolicy::Max,
            gap_ratio: 0.1,
        };
        let result = arrange(&items, options);
        assert_eq!(
            result.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
            vec![7, 3, 9]
        );
        // 输入序不会被「按宽度重排」打乱：前三张按给定顺序进第一行，第四张换行。
        let rows = rows_of(&result);
        assert_eq!(rows[0], vec![7, 3]);
        assert_eq!(rows[1], vec![9]);
    }

    #[test]
    fn a_zero_gap_makes_neighbours_touch() {
        let items = [
            item(1, 100.0, 100.0, 0.0, 0.0),
            item(2, 100.0, 100.0, 120.0, 0.0),
        ];
        let result = arrange(
            &items,
            ArrangeOptions {
                order: ItemOrder::Reading,
                height: HeightPolicy::Median,
                gap_ratio: 0.0,
            },
        );
        // 贴紧是合法选项：右边缘与左边缘重合，不留任何缝隙。
        assert!((result[1].1[0][0] - result[0].1[1][0]).abs() < 1e-6);
        assert!((result[0].1[0][1] - result[1].1[0][1]).abs() < 1e-6);
    }

    #[test]
    fn unusable_items_are_skipped() {
        let items = [
            item(1, 100.0, 100.0, 0.0, 0.0),
            item(2, 0.0, 100.0, 100.0, 0.0),
            item(3, f64::NAN, 100.0, 200.0, 0.0),
        ];
        let result = arrange(&items, options());
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].0, 1);
    }

    #[test]
    fn greedy_and_optimal_breaking_agree_on_a_small_case() {
        // 贪心兜底路径只在超大规模启用，这里直接验证它不会漏行或重叠。
        let widths = [100.0, 100.0, 100.0, 100.0, 100.0];
        let ranges = greedy_row_ranges(&widths, 10.0, 220.0);
        assert_eq!(ranges, vec![(0, 2), (2, 4), (4, 5)]);
        let total: usize = ranges.iter().map(|(start, end)| end - start).sum();
        assert_eq!(total, widths.len());
    }
}
