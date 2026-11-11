// Small DOM helpers. Text always goes in as text nodes, never as HTML.
// Styling is done with classes and attributes only. Inline style is never written as markup,
// because the app's content security policy forbids it.

export type Child = Node | string | null | undefined | false;
export type AttrValue = string | number | boolean | null | undefined;
export type Attrs = Record<string, AttrValue>;

function setAttrs(el: Element, attrs: Attrs): void {
  for (const name of Object.keys(attrs)) {
    const value = attrs[name];
    if (value === null || value === undefined || value === false) {
      el.removeAttribute(name);
    } else {
      el.setAttribute(name, value === true ? "" : String(value));
    }
  }
}

export function append(parent: Element, children: readonly Child[]): void {
  for (const child of children) {
    if (child === null || child === undefined || child === false) continue;
    parent.append(typeof child === "string" ? document.createTextNode(child) : child);
  }
}

export function h<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  attrs: Attrs = {},
  children: readonly Child[] = [],
): HTMLElementTagNameMap[K] {
  const el = document.createElement(tag);
  setAttrs(el, attrs);
  append(el, children);
  return el;
}

/** Writes text only when it changed, so a refresh does not touch unchanged nodes. */
export function setText(el: Element, value: string): void {
  if (el.textContent !== value) el.textContent = value;
}

export function setAttr(el: Element, name: string, value: string | null): void {
  if (value === null) {
    if (el.hasAttribute(name)) el.removeAttribute(name);
  } else if (el.getAttribute(name) !== value) {
    el.setAttribute(name, value);
  }
}

export function show(el: HTMLElement, visible: boolean): void {
  if (el.hidden === visible) el.hidden = !visible;
}

/**
 * Makes `parent` hold exactly `wanted`, in that order. Nodes already in the right place are not
 * touched, so focus and scroll position survive a refresh.
 */
export function syncChildren(parent: Element, wanted: readonly Element[]): void {
  let cursor: Element | null = parent.firstElementChild;
  for (const el of wanted) {
    if (el === cursor) {
      cursor = cursor.nextElementSibling;
    } else {
      parent.insertBefore(el, cursor);
    }
  }
  while (cursor !== null) {
    const next: Element | null = cursor.nextElementSibling;
    parent.removeChild(cursor);
    cursor = next;
  }
}

// Icons. Plain strokes at 16px, drawn with DOM calls so no markup string is parsed.

const SVG_NS = "http://www.w3.org/2000/svg";

export function svg(tag: "svg" | "path" | "circle", attrs: Attrs, children: readonly SVGElement[] = []): SVGElement {
  const el = document.createElementNS(SVG_NS, tag);
  setAttrs(el, attrs);
  for (const child of children) el.append(child);
  return el;
}

function shape(d: string, cls?: string): SVGElement {
  return svg("path", { d, class: cls });
}

function icon(size: number, shapes: readonly SVGElement[], cls?: string): SVGElement {
  return svg(
    "svg",
    {
      viewBox: `0 0 ${size} ${size}`,
      width: size,
      height: size,
      fill: "none",
      stroke: "currentColor",
      "stroke-width": 1.5,
      "stroke-linecap": "round",
      "stroke-linejoin": "round",
      "aria-hidden": "true",
      class: cls,
    },
    shapes,
  );
}

export function iconRefresh(): SVGElement {
  return icon(16, [shape("M13.5 8A5.5 5.5 0 1 1 8 2.5c1.5 0 2.9.6 3.95 1.6L13.5 5.5"), shape("M13.5 2.5v3h-3")]);
}

export function iconPin(): SVGElement {
  return icon(16, [shape("M5.75 2.5h4.5"), shape("M6.75 2.5v3.4l-2 2.5h6.5l-2-2.5V2.5", "pin-body"), shape("M8 8.4v5.1")]);
}

export function iconSliders(): SVGElement {
  return icon(16, [
    shape("M2.5 5h6.25M12.25 5h1.25M2.5 11h1.25M7.25 11h6.25"),
    svg("circle", { cx: 10.5, cy: 5, r: 1.75 }),
    svg("circle", { cx: 5.5, cy: 11, r: 1.75 }),
  ]);
}

export function iconBack(): SVGElement {
  return icon(16, [shape("M10 3.5L5.5 8l4.5 4.5")]);
}

/** The provider's icon, drawn by CSS from `src/assets/providers/<service>.svg` in the provider's colour. */
export function providerMark(service: string, cls = "mark"): HTMLSpanElement {
  return h("span", { class: cls, "data-service": service, "aria-hidden": "true" });
}

// Switch: a real button with role="switch". The whole 44 by 32 area is the hit target.

export interface Switch {
  readonly el: HTMLButtonElement;
  set(checked: boolean): void;
}

export interface SwitchOptions {
  /** id of the element that names this switch. */
  labelledBy?: string;
  /** id of the element that describes this switch. */
  describedBy?: string;
  onChange(next: boolean): void;
}

export function createSwitch(options: SwitchOptions): Switch {
  const thumb = h("span", { class: "switch-thumb" });
  const track = h("span", { class: "switch-track" }, [thumb]);
  const el = h(
    "button",
    {
      class: "switch",
      type: "button",
      role: "switch",
      "aria-checked": "false",
      "aria-labelledby": options.labelledBy,
      "aria-describedby": options.describedBy,
    },
    [track],
  );
  const set = (checked: boolean): void => setAttr(el, "aria-checked", checked ? "true" : "false");
  el.addEventListener("click", () => {
    const next = el.getAttribute("aria-checked") !== "true";
    set(next);
    options.onChange(next);
  });
  return { el, set };
}
