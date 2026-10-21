// Bridge between style.css and three.js: colours are declared once as CSS
// custom properties and read here, so the legend swatches and the 3D
// materials can never drift apart.

export function cssColor(name, fallback) {
  const value = getComputedStyle(document.documentElement).getPropertyValue(name).trim();
  return value || fallback;
}
