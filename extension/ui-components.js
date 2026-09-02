/* AI Continuity Bridge — UI Component Library v4
   Reusable, lightweight DOM components for the extension.
   Edge-docked panel with an activity timeline: one entry per tool call. */

(function () {
  "use strict";

  function el(tag, attrs, children) {
    const e = document.createElement(tag);
    if (attrs) Object.entries(attrs).forEach(([k, v]) => e.setAttribute(k, v));
    if (children) children.forEach((c) => e.appendChild(typeof c === "string" ? document.createTextNode(c) : c));
    return e;
  }

  function escapeHtml(s) {
    return s.replace(/[&<>"']/g, (c) => ({
      "&": "&amp;",
      "<": "&lt;",
      ">": "&gt;",
      '"': "&quot;",
      "'": "&#39;",
    })[c]);
  }

  const CLOSE_SVG = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>';

  // `run_command` output is capped at 1MB by the core (bridge.rs) and
  // `read_file` at 512KB — far more than a floating widget should paint.
  const RENDER_CAP = 128 * 1024;

  /** Trim text to what a widget will render, using the core's own marker. */
  function capText(s) {
    const t = String(s ?? "");
    return t.length > RENDER_CAP ? t.slice(0, RENDER_CAP) + "\n[output truncated]" : t;
  }

  function createCloseBtn() {
    const btn = el("button", { class: "acb-close", title: "Dismiss" });
    btn.innerHTML = CLOSE_SVG;
    return btn;
  }

  /** Entry-header timestamp, e.g. "14:07". */
  function timeNow() {
    const d = new Date();
    return `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
  }

  // Badge icons, looked up by tool name and falling back to the tool's
  // group — so a new tool gets a sensible icon without touching this file.
  const SVG = (body) =>
    `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">${body}</svg>`;

  const ICONS = {
    // by group
    Reading: SVG(
      '<path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><polyline points="14 2 14 8 20 8"/><line x1="16" y1="13" x2="8" y2="13"/><line x1="16" y1="17" x2="8" y2="17"/><polyline points="10 9 9 9 8 9"/>',
    ),
    Editing: SVG(
      '<path d="M11 4H4a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-7"/><path d="M18.5 2.5a2.121 2.121 0 0 1 3 3L12 15l-4 1 1-4 9.5-9.5z"/>',
    ),
    Commands: SVG('<polyline points="4 17 10 11 4 5"/><line x1="12" y1="19" x2="20" y2="19"/>'),
    Search: SVG('<circle cx="11" cy="11" r="7"/><line x1="21" y1="21" x2="16.65" y2="16.65"/>'),
    Git: SVG(
      '<circle cx="12" cy="12" r="4"/><line x1="1.05" y1="12" x2="7" y2="12"/><line x1="17.01" y1="12" x2="22.96" y2="12"/>',
    ),
    Planning: SVG(
      '<polyline points="9 11 12 14 22 4"/><path d="M21 12v7a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h11"/>',
    ),
    Meta: SVG('<circle cx="12" cy="12" r="10"/><line x1="12" y1="16" x2="12" y2="12"/><line x1="12" y1="8" x2="12.01" y2="8"/>'),
    // per-tool overrides
    list_directory: SVG(
      '<path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"/>',
    ),
  };

  const CHEVRON_SVG =
    '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="9 18 15 12 9 6"/></svg>';

  // ── Dock — edge-docked panel with an activity timeline ──────────
  //
  // Collapsed, it is a vertical tab on the right edge showing connection
  // state and the running entry count. Expanded, a panel: header (status),
  // a scrolling timeline of widget entries, and a footer stage line that
  // replaces the old floating stage chip.
  class ACBDock {
    constructor() {
      this.el = el("div", { class: "acb-dock", "data-open": "false", "data-conn": "connecting" });

      // Collapsed: vertical tab on the right edge
      this.tab = el("button", { class: "acb-dock-tab", title: "AI Continuity Bridge" });
      this.tabDot = el("span", { class: "acb-status-dot" });
      this.tabLabel = el("span", { class: "acb-dock-tab-label" }, ["BRIDGE"]);
      this.tabBadge = el("span", { class: "acb-dock-tab-badge" }, ["0"]);
      this.tab.appendChild(this.tabDot);
      this.tab.appendChild(this.tabLabel);
      this.tab.appendChild(this.tabBadge);
      this.tab.addEventListener("click", () => this.open());

      // Expanded: the panel
      this.panel = el("div", { class: "acb-dock-panel" });

      const header = el("div", { class: "acb-dock-header" });
      const title = el("span", { class: "acb-dock-title" }, ["Bridge"]);
      this.statusText = el("span", { class: "acb-dock-status" }, ["connecting…"]);
      const collapse = el("button", { class: "acb-dock-collapse", title: "Collapse" });
      collapse.innerHTML = CHEVRON_SVG;
      collapse.addEventListener("click", () => this.close());
      header.appendChild(title);
      header.appendChild(this.statusText);
      header.appendChild(collapse);
      this.panel.appendChild(header);

      this.timeline = el("div", { class: "acb-dock-timeline" });
      this._empty = el("div", { class: "acb-dock-empty" }, ["No tool activity yet."]);
      this.timeline.appendChild(this._empty);
      this.panel.appendChild(this.timeline);

      this.footer = el("div", { class: "acb-dock-footer", "data-state": "idle" });
      this.footerText = el("span", { class: "acb-dock-footer-text" });
      this.historyBtn = el(
        "button",
        { class: "acb-dock-history-btn", title: "Tool history" },
        ["History"],
      );
      this.footer.appendChild(this.footerText);
      this.footer.appendChild(this.historyBtn);
      this.panel.appendChild(this.footer);

      this.el.appendChild(this.panel);
      this.el.appendChild(this.tab);

      this._stageTimer = null;
      this.count = 0;
      // Entries mount themselves into the timeline (widgets take a root
      // node), so bookkeeping watches for their arrival instead of waiting
      // to be told.
      this._observer = new MutationObserver(() => this._sync());
      this._observer.observe(this.timeline, { childList: true });
    }
    /** Register the handler for the footer's History button. */
    onHistory(cb) {
      this.historyBtn.addEventListener("click", (e) => {
        e.stopPropagation();
        cb();
      });
    }
    open() {
      this.el.setAttribute("data-open", "true");
      this._scroll();
    }
    close() {
      this.el.setAttribute("data-open", "false");
    }
    setStatus(state) {
      this.el.setAttribute("data-conn", state);
      this.statusText.textContent =
        state === "connected" ? "connected" : state === "connecting" ? "connecting…" : "disconnected";
    }
    setStage(text, state) {
      if (this._stageTimer) {
        clearTimeout(this._stageTimer);
        this._stageTimer = null;
      }
      this.footer.setAttribute("data-state", state || "working");
      this.footerText.textContent = text;
      if (state === "done" || state === "error") {
        this._stageTimer = setTimeout(() => {
          this.footer.setAttribute("data-state", "idle");
          this.footerText.textContent = "";
        }, 2500);
      }
    }
    _sync() {
      const kids = this.timeline.children.length;
      if (kids > 0 && this._empty) {
        this._empty.remove();
        this._empty = null;
      }
      // The count only grows — dismissals remove nodes but the tab badge
      // tracks total activity, like an unread counter for the session.
      if (kids > this.count) {
        this.count = kids;
        this.el.setAttribute("data-fresh", "true");
        setTimeout(() => this.el.removeAttribute("data-fresh"), 1000);
      }
      this.tabBadge.textContent = String(this.count);
      this._scroll();
    }
    _scroll() {
      this.timeline.scrollTop = this.timeline.scrollHeight;
    }
    mount(root) {
      root.appendChild(this.el);
      return this;
    }
  }

  // ── Tool Card (approval pending) ─────────────────────────────────
  //
  // Tool objects arrive in three shapes — a parsed v2 call, a v2 result's
  // `meta`, and the legacy v1 serde enum — so normalization goes through
  // the shared spec table rather than a per-shape ladder here.
  class ACBToolCard {
    constructor(tool, msgId) {
      this.tool = tool;
      this.norm = (window.ACBToolSpec && window.ACBToolSpec.normalizeTool(tool)) || {
        name: "unknown",
        args: {},
        spec: null,
      };
      this.msgId = msgId;
      this.el = el("div", {
        class: "acb-widget acb-tool-card",
        "data-state": "pending",
        "data-tool": this._toolName(),
        "data-expanded": "true",
      });
      this._build();
    }
    _toolName() {
      return this.norm.name;
    }
    /** The path or command the call acts on, for the card header. */
    _toolDetail() {
      const { args, spec } = this.norm;
      if (spec) {
        for (const arg of spec.args) {
          if (arg.multiline) continue;
          if (typeof args[arg.name] === "string") return args[arg.name];
        }
        // A v2 `meta` carries a pre-formatted detail when it has no args.
        if (typeof args.detail === "string") return args.detail;
        return spec.args.length === 0 ? `(${spec.group.toLowerCase()})` : "";
      }
      return args.path || args.command || args.detail || "";
    }
    _toolIcon() {
      return ICONS[this._toolName()] || ICONS[this.norm.spec?.group] || "";
    }
    _build() {
      const name = this._toolName();
      const detail = this._toolDetail();
      const iconHtml = `<span class="acb-tool-badge-icon">${this._toolIcon()}</span>`;
      // Colour comes from the tool's group, so a new tool needs no CSS;
      // the name class is still emitted for per-tool overrides.
      const group = this.norm.spec ? `group-${this.norm.spec.group.toLowerCase()}` : "";

      // Header — click to expand/collapse, close button
      const header = el("div", { class: "acb-widget-header" });
      header.innerHTML = `
        <span class="acb-tool-badge ${group} ${name}">${iconHtml}${name}</span>
        <span class="acb-tool-path">${escapeHtml(detail)}</span>
        <span class="acb-widget-time">${timeNow()}</span>
      `;
      this._closeBtn = createCloseBtn();
      header.appendChild(this._closeBtn);
      this.el.appendChild(header);

      // Body — preview + actions
      const body = el("div", { class: "acb-widget-body" });

      // Show what the call will do: the content to be written, or the
      // command to be run. Nothing is more important on an approval card.
      const multiline = this.norm.spec?.args.find((a) => a.multiline);
      const preview = multiline ? this.norm.args[multiline.name] : null;
      if (typeof preview === "string") {
        const previewEl = el("div", { class: "acb-tool-preview" }, []);
        previewEl.textContent = capText(preview);
        body.appendChild(previewEl);
      } else if (name === "run_command" && detail) {
        const cmdEl = el("div", { class: "acb-tool-preview" });
        cmdEl.innerHTML = `<span style="color:var(--acb-text-dim)">$</span> ${escapeHtml(detail)}`;
        body.appendChild(cmdEl);
      }

      // Approvals resolve in the desktop app only. Allow/Deny buttons here
      // would live in the host page's DOM, where any page script could
      // approve via a synthetic `.click()` — no isTrusted check can make
      // that safe — so this card is informational.
      const note = el("div", { class: "acb-tool-note" });
      note.textContent = "Approval required — allow or deny in the desktop app";
      body.appendChild(note);
      this.el.appendChild(body);
    }
    onAction(cb) {
      // Close button
      this._closeBtn.addEventListener("click", (e) => {
        e.stopPropagation();
        this.el.setAttribute("data-state", "dismissed");
        setTimeout(() => this.el.remove(), 200);
        cb("dismiss");
      });
      // Expand/collapse on header click (not close button)
      this.el.querySelector(".acb-widget-header").addEventListener("click", (e) => {
        if (e.target.closest(".acb-close")) return;
        const expanded = this.el.getAttribute("data-expanded") === "true";
        this.el.setAttribute("data-expanded", expanded ? "false" : "true");
      });
    }
    mount(root) {
      root.appendChild(this.el);
      return this;
    }
  }

  // ── Result Block ─────────────────────────────────────────────────
  class ACBResultBlock {
    constructor(result, toolName, opts) {
      this.result = result;
      this.toolName = toolName;
      this.detail = (opts && opts.detail) || "";
      this.errorCode = (opts && opts.errorCode) || "";
      this.el = el("div", {
        class: "acb-widget acb-result-block",
        "data-state": result.ok ? "success" : "error",
        // Errors expand fully so the real cause is visible immediately.
        "data-expanded": result.ok ? "false" : "true",
      });
      this._build();
    }
    _build() {
      const ok = this.result.ok;
      const output = this.result.output || this.result.error || "";

      // Header — click to expand/collapse, close button
      const header = el("div", { class: "acb-widget-header" });
      const checkSvg = ok
        ? '<svg class="acb-result-check" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><circle cx="12" cy="12" r="10"/><path d="M8 12l2.5 2.5L16 9.5"/></svg>'
        : '<svg class="acb-result-x" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><circle cx="12" cy="12" r="10"/><path d="M15 9l-6 6M9 9l6 6"/></svg>';
      const label = ok ? "Done" : "Failed";
      const metaParts = [this.toolName, this.detail].filter(Boolean).join(" ");
      const codeHtml = this.errorCode
        ? `<span class="acb-result-code">${escapeHtml(this.errorCode)}</span>`
        : "";
      header.innerHTML = `${checkSvg}<span class="acb-result-label">${label}</span><span class="acb-result-meta">${escapeHtml(metaParts)}</span>${codeHtml}<span class="acb-widget-time">${timeNow()}</span>`;
      this._closeBtn = createCloseBtn();
      header.appendChild(this._closeBtn);
      this.el.appendChild(header);

      // Body — content + actions
      const body = el("div", { class: "acb-widget-body" });

      const content = el("div", { class: "acb-result-content" });
      const pre = el("pre");
      const code = el("code");
      code.textContent = capText(output);
      pre.appendChild(code);
      content.appendChild(pre);
      body.appendChild(content);

      const actions = el("div", { class: "acb-result-actions" });
      actions.innerHTML = `<button class="acb-btn acb-btn--ghost acb-btn--sm" data-action="insert">Insert into chat</button>`;
      body.appendChild(actions);
      this.el.appendChild(body);
    }
    onAction(cb) {
      // Close button
      this._closeBtn.addEventListener("click", (e) => {
        e.stopPropagation();
        this.el.setAttribute("data-state", "dismissed");
        setTimeout(() => this.el.remove(), 200);
        cb("dismiss");
      });
      // Expand/collapse on header click
      this.el.querySelector(".acb-widget-header").addEventListener("click", (e) => {
        if (e.target.closest(".acb-close")) return;
        const expanded = this.el.getAttribute("data-expanded") === "true";
        this.el.setAttribute("data-expanded", expanded ? "false" : "true");
      });
      // Insert button
      this.el.addEventListener("click", (e) => {
        const btn = e.target.closest("[data-action]");
        if (btn) cb(btn.getAttribute("data-action"));
      });
    }
    mount(root) {
      root.appendChild(this.el);
      return this;
    }
  }

  // ── Terminal Stream ──────────────────────────────────────────────
  class ACBTerminal {
    constructor(command, msgId) {
      this.command = command;
      this.msgId = msgId;
      this.el = el("div", {
        class: "acb-widget acb-terminal",
        "data-expanded": "false",
      });
      this._build();
    }
    _build() {
      // Header — click to expand/collapse, close button
      this.header = el("div", { class: "acb-widget-header acb-terminal-header" });
      const headerInner = el("div", { style: "display:flex;align-items:center;gap:8px;flex:1;min-width:0;" });
      headerInner.innerHTML = `
        <span class="acb-terminal-prompt">$ ${escapeHtml(this.command)}</span>
        <span class="acb-terminal-status running">Running…</span>
        <span class="acb-widget-time">${timeNow()}</span>
      `;
      this.header.appendChild(headerInner);
      // Stop button — the core's process registry kills the process group
      // this request spawned; the button only sends the `cancel` frame. It
      // exists solely while the command is in flight, so finish() removes it.
      this.stopBtn = el(
        "button",
        { class: "acb-btn acb-btn--deny acb-btn--sm acb-terminal-stop", title: "Kill this command" },
        ["Stop"],
      );
      this.stopBtn.addEventListener("click", (e) => {
        // Without this the click bubbles to the header and toggles the
        // expand/collapse the same click was fighting with.
        e.stopPropagation();
        if (this.stopBtn.disabled) return;
        this.stopBtn.disabled = true;
        this.setNote("Stopping…");
        if (this._onStop) this._onStop();
      });
      headerInner.appendChild(this.stopBtn);
      this._closeBtn = createCloseBtn();
      this.header.appendChild(this._closeBtn);
      this.el.appendChild(this.header);

      // Body
      const body = el("div", { class: "acb-widget-body" });

      this.output = el("div", { class: "acb-terminal-output" });
      body.appendChild(this.output);

      this.footer = el("div", { class: "acb-terminal-footer" }, ["Waiting for output…"]);
      body.appendChild(this.footer);
      this.el.appendChild(body);
    }
    /**
     * Render the whole output at once. Callers have the complete text, so
     * this is the normal path; `appendLine` exists for streaming.
     */
    setOutput(text) {
      this._buf = String(text ?? "");
      this._flush();
    }
    appendLine(text) {
      this._buf = (this._buf ?? "") + text + "\n";
      this._flush();
    }
    /**
     * One DOM write and one layout per frame. Reading `scrollHeight` right
     * after writing `textContent` forces a synchronous reflow, so doing it
     * per line was O(n²) — ~20k forced layouts on a 1MB command output, which
     * pegged the CPU and made the host page unresponsive.
     */
    _flush() {
      if (this._raf) return;
      this._raf = requestAnimationFrame(() => {
        this._raf = null;
        this.output.textContent = capText(this._buf);
        this.output.scrollTop = this.output.scrollHeight;
      });
    }
    /** Replace the status line while the command is still in flight. */
    setNote(text) {
      const status = this.header.querySelector(".acb-terminal-status");
      if (status) status.textContent = text;
    }
    finish(ok, elapsed) {
      const status = this.header.querySelector(".acb-terminal-status");
      status.className = `acb-terminal-status ${ok ? "done" : "error"}`;
      status.textContent = ok ? "Done" : "Failed";
      this.footer.textContent = `Exit code: ${ok ? "0" : "1"} • ${elapsed || "?"}`;
      // Nothing left to stop once the result is in.
      if (this.stopBtn) this.stopBtn.remove();
    }
    /** Register the handler for the Stop button (sends the cancel frame). */
    onStop(cb) {
      this._onStop = cb;
    }
    onAction(cb) {
      // Close button
      this._closeBtn.addEventListener("click", (e) => {
        e.stopPropagation();
        this.el.setAttribute("data-state", "dismissed");
        setTimeout(() => this.el.remove(), 200);
        cb("dismiss");
      });
      // Expand/collapse on header click
      this.header.addEventListener("click", (e) => {
        if (e.target.closest(".acb-close")) return;
        const expanded = this.el.getAttribute("data-expanded") === "true";
        this.el.setAttribute("data-expanded", expanded ? "false" : "true");
      });
      // Insert button
      this.el.addEventListener("click", (e) => {
        const btn = e.target.closest("[data-action]");
        if (btn) cb(btn.getAttribute("data-action"));
      });
    }
    mount(root) {
      root.appendChild(this.el);
      return this;
    }
  }

  // ── Handoff Card ─────────────────────────────────────────────────
  class ACBHandoffCard {
    constructor(handoff, targetLabel) {
      this.handoff = handoff;
      this.targetLabel = targetLabel;
      this.overlay = el("div", { class: "acb-handoff-overlay" });
      this._build();
    }
    _build() {
      const card = el("div", { class: "acb-handoff-card" });
      const svg =
        '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M22 11.08V12a10 10 0 1 1-5.93-9.14"/><polyline points="22 4 12 14.01 9 11.01"/></svg>';

      card.innerHTML = `
        <h3>${svg} Bridge handoff ready</h3>
        <div class="acb-handoff-objective">${escapeHtml(this.handoff.objective)}</div>
        <div class="acb-handoff-stats">
          <span class="acb-handoff-stat progress"><b>${this.handoff.progress_percent}%</b> progress</span>
          <span class="acb-handoff-stat"><b>${this.handoff.files_changed}</b> files changed</span>
          <span class="acb-handoff-stat errors"><b>${this.handoff.errors_remaining}</b> errors</span>
        </div>
        ${this.handoff.next_step ? `<div class="acb-handoff-next">Next: ${escapeHtml(this.handoff.next_step)}</div>` : ""}
        <div class="acb-handoff-actions">
          <button class="acb-btn acb-btn--deny" data-action="dismiss">Dismiss</button>
          <button class="acb-btn acb-btn--allow" data-action="continue">${this.targetLabel}</button>
        </div>
      `;
      this.overlay.appendChild(card);
      this.card = card;
    }
    onAction(cb) {
      this.overlay.addEventListener("click", (e) => {
        const btn = e.target.closest("[data-action]");
        if (btn) {
          cb(btn.getAttribute("data-action"));
          this.destroy();
        }
      });
      this.overlay.addEventListener("click", (e) => {
        if (e.target === this.overlay) {
          this.destroy();
        }
      });
    }
    mount(root) {
      root.appendChild(this.overlay);
      return this;
    }
    destroy() {
      this.overlay.remove();
    }
  }

  // ── History Panel ────────────────────────────────────────────────
  //
  // Persistent log of past tool calls, kept by the background script in
  // chrome.storage — the extension's counterpart to the desktop app's
  // audit trail. Read-only: entries render exactly what the background
  // recorded, newest first, output expanded by clicking the row.
  const HISTORY_STATUS = {
    running: "running…",
    pending: "awaiting",
    success: "done",
    error: "error",
    timeout: "timeout",
    denied: "denied",
  };

  class ACBHistoryPanel {
    constructor(entries) {
      this.entries = Array.isArray(entries) ? entries : [];
      this.el = el("div", { class: "acb-history" });
      this._build();
    }
    _build() {
      const header = el("div", { class: "acb-history-header" });
      header.appendChild(el("span", { class: "acb-dock-title" }, ["History"]));
      header.appendChild(
        el("span", { class: "acb-history-count" }, [`${this.entries.length} calls`]),
      );
      const close = createCloseBtn();
      close.addEventListener("click", () => this.destroy());
      header.appendChild(close);
      this.el.appendChild(header);

      const list = el("div", { class: "acb-history-list" });
      if (this.entries.length === 0) {
        list.appendChild(el("div", { class: "acb-dock-empty" }, ["No history yet."]));
      }
      // Newest first — the freshest call is what the user is after.
      for (const entry of [...this.entries].reverse()) {
        list.appendChild(this._entry(entry));
      }
      this.el.appendChild(list);
    }
    _entry(entry) {
      const row = el("div", {
        class: "acb-history-entry",
        "data-status": entry.status || "running",
        "data-expanded": "false",
      });
      const ts = entry.ts
        ? new Date(entry.ts).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })
        : "";
      const head = el("div", { class: "acb-history-entry-head" });
      head.innerHTML = `
        <span class="acb-tool-badge-icon">${ICONS[entry.tool] || ICONS.Search || ""}</span>
        <span class="acb-history-name">${escapeHtml(entry.tool || "tool")}</span>
        <span class="acb-history-detail">${escapeHtml(entry.detail || "")}</span>
        <span class="acb-history-time">${ts}</span>
        <span class="acb-history-status">${escapeHtml(HISTORY_STATUS[entry.status] || entry.status || "")}</span>
      `;
      row.appendChild(head);
      if (entry.output) {
        const out = el("div", { class: "acb-history-output" });
        out.textContent = capText(entry.output);
        row.appendChild(out);
      }
      head.addEventListener("click", () => {
        if (!entry.output) return;
        const expanded = row.getAttribute("data-expanded") === "true";
        row.setAttribute("data-expanded", expanded ? "false" : "true");
      });
      return row;
    }
    mount(root) {
      root.appendChild(this.el);
      return this;
    }
    destroy() {
      this.el.remove();
    }
  }

  // ── Exports ──────────────────────────────────────────────────────
  window.ACBComponents = {
    ACBDock,
    ACBToolCard,
    ACBResultBlock,
    ACBTerminal,
    ACBHandoffCard,
    ACBHistoryPanel,
    escapeHtml,
  };
})();
