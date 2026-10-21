// Sidebar: a filterable host list and a details card for the selected host.
//
// The details card is generic on purpose. Apart from a few keys that get a
// friendlier heading, every field of the host JSON is rendered as it comes
// (scalars as rows, lists of objects as tables, objects as nested rows), so a
// field added to pynmapper/model.py shows up here with no frontend change.

const GRAPHICS_KEYS = new Set(["id", "label", "role", "position"]); // frontend-only keys, not shown
const HEADINGS = { ip: "IP", hostname: "hostname", open_ports: "open ports", is_local: "local", extra: "extra" };
const ROLE_ORDER = { scanner: 0, gateway: 1, host: 2 };

export class Panel {
  constructor(root, { onSelect, onFly }) {
    this.root = root;
    this.onSelect = onSelect;   // (id | null, { fly }) => void
    this.onFly = onFly;         // () => void
    this.filterInput = root.querySelector("#filter");
    this.countEl = root.querySelector("#host-count");
    this.listEl = root.querySelector("#hosts");
    this.detailsEl = root.querySelector("#details");

    this.hosts = [];
    this.selectedId = null;
    this.query = "";

    this.filterInput.addEventListener("input", () => {
      this.query = this.filterInput.value.trim().toLowerCase();
      this.renderList();
    });
    this.filterInput.addEventListener("keydown", (e) => {
      if (e.key !== "Enter") return;
      const first = this.visibleHosts()[0];
      if (first) this.onSelect(first.id, { fly: true });
    });
    this.listEl.addEventListener("click", (e) => {
      const li = e.target.closest("li[data-id]");
      if (li) this.onSelect(li.dataset.id, { fly: true });
    });
    this.detailsEl.addEventListener("click", (e) => {
      const action = e.target.closest("[data-action]")?.dataset.action;
      if (action === "fly") this.onFly();
      else if (action === "close") this.onSelect(null, {});
    });
    root.querySelector("#sidebar-toggle")?.addEventListener("click", () => this.toggle());
  }

  toggle(force) {
    this.root.classList.toggle("collapsed", force);
  }

  focusFilter() {
    this.toggle(false);
    this.filterInput.focus();
    this.filterInput.select();
  }

  // ----- data ---------------------------------------------------------------------
  setHosts(hosts) {
    this.hosts = [...hosts].sort(compareHosts);
    this.renderList();
    this.renderDetails();
  }

  setSelected(id) {
    this.selectedId = id ?? null;
    for (const li of this.listEl.children) {
      const on = li.dataset.id === this.selectedId;
      li.classList.toggle("selected", on);
      if (on) li.scrollIntoView({ block: "nearest" });
    }
    this.renderDetails();
  }

  visibleHosts() {
    if (!this.query) return this.hosts;
    return this.hosts.filter((h) => searchText(h).includes(this.query));
  }

  // ----- rendering ---------------------------------------------------------------
  renderList() {
    const shown = this.visibleHosts();
    const frag = document.createDocumentFragment();
    for (const host of shown) {
      const li = el("li", { "data-id": host.id, title: `${host.label} (${host.id}) · ${host.role}` });
      li.classList.toggle("selected", host.id === this.selectedId);
      li.append(el("span", { class: `dot role-${host.role}` }), el("span", { class: "name" }, host.label || host.id));
      if (host.label !== host.id) li.append(el("span", { class: "ip" }, host.id));
      frag.append(li);
    }
    this.listEl.replaceChildren(frag);
    this.countEl.textContent = this.query ? `${shown.length} / ${this.hosts.length}` : `${this.hosts.length}`;
  }

  renderDetails() {
    const host = this.hosts.find((h) => h.id === this.selectedId);
    if (!host) {
      this.detailsEl.hidden = true;
      this.detailsEl.replaceChildren();
      return;
    }
    const header = el("header", {},
      el("h2", {}, host.label || host.id),
      el("div", { class: "actions" },
        el("button", { type: "button", "data-action": "fly", title: "fly to this host (F)" }, "fly to"),
        el("button", { type: "button", "data-action": "close", title: "deselect (Esc)" }, "✕"),
      ),
    );
    const sub = el("div", { class: "sub" }, host.id, " · ", el("span", { class: `badge role-${host.role}` }, host.role));
    const fields = [];
    for (const [key, value] of Object.entries(host)) {
      if (GRAPHICS_KEYS.has(key) || key === "ip") continue; // ip is already in the subtitle
      const rendered = renderValue(value);
      fields.push(el("div", { class: isBlock(rendered) ? "field block" : "field" },
        el("div", { class: "key" }, HEADINGS[key] ?? humanize(key)),
        el("div", { class: "value" }, rendered),
      ));
    }
    this.detailsEl.replaceChildren(header, sub, ...fields);
    this.detailsEl.hidden = false;
    this.detailsEl.scrollTop = 0;
  }
}

// ----- helpers -------------------------------------------------------------------------

function el(tag, attrs = {}, ...children) {
  const node = document.createElement(tag);
  for (const [name, value] of Object.entries(attrs)) node.setAttribute(name, value);
  node.append(...children);
  return node;
}

function humanize(key) {
  return String(key).replaceAll("_", " ");
}

function isPlainObject(value) {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function isBlock(node) {
  return node instanceof Element && (node.tagName === "TABLE" || node.classList.contains("nested"));
}

function renderValue(value) {
  if (value === null || value === undefined) return el("span", { class: "muted" }, "—");
  if (typeof value === "boolean") return document.createTextNode(value ? "yes" : "no");
  if (typeof value !== "object") return document.createTextNode(String(value));
  if (Array.isArray(value)) {
    if (value.length === 0) return el("span", { class: "muted" }, "none");
    if (value.every(isPlainObject)) return renderTable(value);
    const span = el("span");
    value.forEach((item, i) => {
      if (i) span.append(", ");
      span.append(renderValue(item));
    });
    return span;
  }
  const keys = Object.keys(value);
  if (keys.length === 0) return el("span", { class: "muted" }, "none");
  const nested = el("div", { class: "nested" });
  for (const key of keys) {
    nested.append(el("div", { class: "key" }, humanize(key)), el("div", { class: "value" }, renderValue(value[key])));
  }
  return nested;
}

// A list of objects (open_ports today, anything similar tomorrow) becomes a
// table whose columns are the union of the objects' keys.
function renderTable(rows) {
  const columns = [...new Set(rows.flatMap((row) => Object.keys(row)))];
  const thead = el("thead", {}, el("tr", {}, ...columns.map((c) => el("th", {}, humanize(c)))));
  const tbody = el("tbody");
  for (const row of rows) {
    tbody.append(el("tr", {}, ...columns.map((c) => el("td", {}, renderValue(row[c])))));
  }
  return el("table", {}, thead, tbody);
}

function searchText(host) {
  const ports = (host.open_ports ?? []).map((p) => `${p.number}/${p.proto}`);
  return [host.label, host.id, host.hostname, host.role, ...ports].filter(Boolean).join(" ").toLowerCase();
}

// Dotted IPv4 sorts numerically; anything else (IPv6, names) falls back to text.
function addressKey(id) {
  const s = String(id);
  return /^\d+(\.\d+){3}$/.test(s) ? s.split(".").map((n) => n.padStart(3, "0")).join(".") : s;
}

function compareHosts(a, b) {
  const byRole = (ROLE_ORDER[a.role] ?? 9) - (ROLE_ORDER[b.role] ?? 9);
  if (byRole) return byRole;
  return addressKey(a.id).localeCompare(addressKey(b.id));
}
