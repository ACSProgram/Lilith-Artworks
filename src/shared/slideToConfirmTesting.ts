import { fireEvent, screen } from "@testing-library/react";
import { SLIDE_FALLBACK_TRAVEL } from "./SlideToConfirm";

/**
 * 驱动滑动确认控件（`SlideToConfirm.tsx`）的测试助手。
 *
 * jsdom 没有实现 `PointerEvent`，但 React 按事件类型分发，所以用带坐标的
 * `MouseEvent` 触发 `pointer*` 监听即可驱动控件里的同一套逻辑。凡是涉及
 * 滑动确认的测试统一从这里取，不要再各写一份拖拽序列。
 */

/** 构造一个能驱动滑动控件的指针事件。 */
export const slidePointer = (type: string, clientX = 0) => new MouseEvent(type, { bubbles: true, clientX });

/** 取滑块本体；`name` 是控件上的可访问名称，即轨迹文案。 */
export function slideThumb(name?: string) {
  return name ? screen.getByRole("slider", { name }) : screen.getByRole("slider");
}

/**
 * 沿轨迹把滑块拖到底再松手：这是滑动确认唯一会触发回调的路径。
 *
 * 行程取控件量不到布局时使用的回退值（jsdom 里 `clientWidth`/`offsetWidth`
 * 都是 0，控件必然走这条分支），因此拖到底的判定与真实布局一致。
 */
export function dragSlideToEnd(name?: string, travel = SLIDE_FALLBACK_TRAVEL) {
  const thumb = slideThumb(name);
  fireEvent(thumb, slidePointer("pointerdown", 0));
  fireEvent(window, slidePointer("pointermove", travel));
  fireEvent(window, slidePointer("pointerup", travel));
  return thumb;
}
