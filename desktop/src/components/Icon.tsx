import "./icon.css";

export type IconName =
  | "chevron-right"
  | "chevron-down"
  | "layout-sidebar-left"
  | "refresh";

// Hand-drawn on the Nerd Font codicon 16 px grid (1.5 px strokes, currentColor,
// no fill), so the UI needs no icon font, no dependency and no network. The
// refresh circle follows the Feather Icons (MIT) 24 px geometry, rescaled.
const PATHS: Record<IconName, string> = {
  "chevron-right": "M6.2 3.6 10.6 8l-4.4 4.4",
  "chevron-down": "M3.6 6.2 8 10.6l4.4-4.4",
  "layout-sidebar-left":
    "M3.5 2.5h9a1 1 0 0 1 1 1v9a1 1 0 0 1-1 1h-9a1 1 0 0 1-1-1v-9a1 1 0 0 1 1-1ZM6.5 2.5v11",
  refresh:
    "M15.33 2.67v4h-4M0.67 13.33v-4h4M2.34 6a6 6 0 0 1 9.9-2.24l3.09 2.91M0.67 9.33l3.09 2.91A6 6 0 0 0 13.66 10",
};

export interface IconProps {
  name: IconName;
  /** Extra class, e.g. `rotated` for the fold chevron. */
  className?: string;
  size?: number;
}

/**
 * A small decorative icon. It is always `aria-hidden`: the button or row that
 * holds it owns the accessible name.
 */
export function Icon({ name, className, size = 16 }: IconProps) {
  return (
    <svg
      className={`icon-svg${className ? ` ${className}` : ""}`}
      width={size}
      height={size}
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.5}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      focusable="false"
    >
      <path d={PATHS[name]} />
    </svg>
  );
}
