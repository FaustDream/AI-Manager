//! 托盘状态图标：运行期按颜色画圆，不引入图片资产与解析器。
//! 32×32 RGBA，1px 抗锯齿边；空心环用于「已停止」，实心圆用于其余三态。

use tray_icon::Icon;

const SIZE: u32 = 32;

/// 画一个抗锯齿圆的 RGBA 缓冲（hollow = 空心环）。
fn draw_circle(color: [u8; 3], hollow: bool) -> Vec<u8> {
    let mut rgba = vec![0u8; (SIZE * SIZE * 4) as usize];
    let center = (SIZE as f32 - 1.0) / 2.0;
    let r_out = SIZE as f32 * 0.46;
    let r_in = if hollow { SIZE as f32 * 0.34 } else { 0.0 };
    for y in 0..SIZE {
        for x in 0..SIZE {
            let dx = x as f32 - center;
            let dy = y as f32 - center;
            let d = (dx * dx + dy * dy).sqrt();
            // 边缘 1px 线性过渡，托盘缩放后不毛糙
            let edge = |r: f32| -> f32 {
                if d <= r - 0.5 {
                    1.0
                } else if d >= r + 0.5 {
                    0.0
                } else {
                    (r + 0.5 - d).clamp(0.0, 1.0)
                }
            };
            let alpha = edge(r_out) - edge(r_in);
            if alpha > 0.0 {
                let i = ((y * SIZE + x) * 4) as usize;
                rgba[i] = color[0];
                rgba[i + 1] = color[1];
                rgba[i + 2] = color[2];
                rgba[i + 3] = (alpha * 255.0) as u8;
            }
        }
    }
    rgba
}

/// 实心圆（运行=绿 / 切换中=黄 / 失败=红）
pub fn filled(color: [u8; 3]) -> Icon {
    Icon::from_rgba(draw_circle(color, false), SIZE, SIZE).expect("图标尺寸固定，构造不会失败")
}

/// 空心环（已停止=灰）
pub fn ring(color: [u8; 3]) -> Icon {
    Icon::from_rgba(draw_circle(color, true), SIZE, SIZE).expect("图标尺寸固定，构造不会失败")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filled_circle_has_opaque_center() {
        let rgba = draw_circle([34, 197, 94], false);
        let center = ((SIZE / 2) * SIZE + SIZE / 2) as usize * 4;
        assert_eq!(rgba[center], 34);
        assert_eq!(rgba[center + 1], 197);
        assert_eq!(rgba[center + 3], 255);
        // 角落在圆外
        let corner = 0;
        assert_eq!(rgba[corner + 3], 0);
    }

    #[test]
    fn hollow_ring_is_transparent_in_the_middle() {
        let rgba = draw_circle([113, 128, 150], true);
        let center = ((SIZE / 2) * SIZE + SIZE / 2) as usize * 4;
        assert_eq!(rgba[center + 3], 0, "环心应透明");
        // 环身上有实色：y=4 在中轴上方 11.5px，落在内外半径（10.88 ~ 14.72）之间
        let y = 4usize;
        let top = (y * SIZE as usize + SIZE as usize / 2) * 4;
        assert!(rgba[top + 3] > 200, "环身应接近不透明");
    }

    #[test]
    fn buffer_shape_is_valid() {
        let rgba = draw_circle([0, 0, 0], false);
        assert_eq!(rgba.len(), (SIZE * SIZE * 4) as usize);
    }
}
