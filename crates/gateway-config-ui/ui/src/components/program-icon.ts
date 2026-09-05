// The program icon (the orange P on the dark shield) as the tab bar, the
// key prompt, and the About card show it. The two files under ui/icons/
// are copies of the workshop icon set's 128x128 and 128x128@2x renders;
// the @2x entry in srcset keeps the image crisp on high-DPI displays.

/** Relative path of the 1x program icon, resolved under the SPA mount. */
export const PROGRAM_ICON = "icons/promptforge-icon.png";

/** Relative path of the 2x program icon, paired with {@link PROGRAM_ICON}. */
export const PROGRAM_ICON_2X = "icons/promptforge-icon@2x.png";

/** Builds an `<img>` of the program icon, `size` CSS pixels square. */
export function programIcon(size: number, alt: string): HTMLImageElement {
  const image = document.createElement("img");
  image.src = PROGRAM_ICON;
  image.srcset = `${PROGRAM_ICON} 1x, ${PROGRAM_ICON_2X} 2x`;
  image.alt = alt;
  image.width = size;
  image.height = size;
  return image;
}
