export const IS_TOUCH_DEVICE =
	typeof window !== "undefined" &&
	(window.matchMedia("(pointer: coarse)").matches || "ontouchstart" in window || navigator.maxTouchPoints > 0);
