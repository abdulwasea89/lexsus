// AI Continuity Bridge — content script v2.0 for the non-ChatGPT web AIs:
// claude.ai, gemini.google.com and grok.com.
//
// The tool vocabulary — names, aliases, parsers, timeouts, prompt text —
// lives in tool-spec.js, shared with content.js (chatgpt.com) and
// background.js. Only per-host DOM handling belongs in this file.

(() => {
  "use strict";

  // Same guard as content.js: the manifest injects at document_idle and the
  // background injects programmatically when a tabs.sendMessage fails during
  // the page-load race. A second copy would re-scan the transcript with its
  // own dedup set — every tool call sent twice, executed twice.
  if (window.__ACB_CONTENT) return;
  window.__ACB_CONTENT = true;

  const host = (() => {
    const h = location.hostname;
    if (h.includes("claude.ai")) return "claudeai";
    if (h.includes("gemini.google.com")) return "gemini";
    if (h.includes("grok.com")) return "grok";
    return "other";
  })();

  // tool-spec.js, ui-components.js and styles.css are injected automatically
  // by the manifest's content_scripts declaration (they run before this).
  const C = window.ACBComponents;
  const S = window.ACBToolSpec;

  // ── DOM selectors per host ──────────────────────────────────────
  //
  // Each value is a comma-separated candidate list, most specific first, so a
  // site redesign degrades instead of breaking outright. COMPS and SUBMIT may
  // end in a generic fallback; MESSAGES must NOT — `scan()` reads the last
  // matching node, so a selector loose enough to match the composer would make
  // our own inserted results re-trigger themselves.
  const COMPS = {
    claudeai: 'div[contenteditable="true"]',
    gemini: 'div.ql-editor[contenteditable="true"], rich-textarea div[contenteditable="true"], div[contenteditable="true"]',
    grok: 'form textarea, textarea[aria-label], main textarea, textarea, div[contenteditable="true"]',
  };
  const SUBMIT = {
    claudeai: 'button[aria-label="Send message"], button[aria-label="Send"]',
    gemini: 'button[aria-label="Send message"], button[data-test-id="send-button"]',
    grok: 'button[type="submit"], button[aria-label="Submit"], button[aria-label="Send message"]',
  };
  const MESSAGES = {
    claudeai: ".font-claude-message",
    gemini: "model-response .markdown-content, .model-response-text, .markdown-content",
    grok: ".response-content-markdown, .message-bubble, [data-testid='message-content']",
  };

  // ── Handoff prompt ──────────────────────────────────────────────
  const HANDOFF_PROMPT = (h) =>
    [
      `# Continue this task (AI Continuity Bridge handoff)`,
      ``,
      `Objective: ${h.objective}`,
      `Progress: ${h.progress_percent}% · Files changed: ${h.files_changed} · Errors remaining: ${h.errors_remaining}`,
      `Next step: ${h.next_step ?? "review the project state"}`,
      h.files && h.files.length > 0 ? `Files involved: ${h.files.join(", ")}` : "",
      h.context ? `Task context so far: ${h.context}` : "",
      h.end_reason ? `Where the previous session stopped: ${h.end_reason}` : "",
      ``,
      S.promptToolSection(),
    ]
      .filter(Boolean)
      .join("\n");

  // ── Composer helpers ────────────────────────────────────────────
  function findComposer() {
    const sel = COMPS[host] || "div[contenteditable='true']";
    return (
      document.querySelector(sel) ||
      document.querySelector("textarea") ||
      document.querySelector("#prompt-textarea")
    );
  }

  function insertIntoComposer(text) {
    const el = findComposer();
    if (!el) return false;
    // Every insert path funnels through here, so one cap covers auto-insert,
    // both Insert buttons, the handoff prompt and the manifest button.
    text = S.capForComposer(text);
    el.focus();
    const sel = window.getSelection();
    if (sel && sel.rangeCount > 0) {
      sel.selectAllChildren(el);
      sel.collapseToEnd();
    }
    if (el.tagName === "TEXTAREA") {
      const setter = Object.getOwnPropertyDescriptor(
        window.HTMLTextAreaElement.prototype,
        "value",
      ).set;
      setter.call(el, el.value + text);
      el.dispatchEvent(new Event("input", { bubbles: true }));
      el.dispatchEvent(new Event("change", { bubbles: true }));
    } else {
      document.execCommand("insertText", false, text);
      el.dispatchEvent(new Event("input", { bubbles: true }));
    }
    return true;
  }

  function submitComposer() {
    const sel = SUBMIT[host];
    const btn = sel && document.querySelector(sel);
    if (btn) {
      btn.click();
      return true;
    }
    const el = findComposer();
    if (el) {
      el.dispatchEvent(
        new KeyboardEvent("keydown", {
          key: "Enter",
          code: "Enter",
          keyCode: 13,
          which: 13,
          bubbles: true,
        }),
      );
      return true;
    }
    return false;
  }

  // ── Scanning ────────────────────────────────────────────────────
  const sentSigs = new Set();
  // request id → { terminal, output } for an in-flight run_command. The
  // background generates the id, so the terminal (and its Stop button)
  // can only mount once the sendMessage round-trip returns it.
  const runningTerms = new Map();

  function sendTool(tool) {
    if (!tool) return;
    const sig = JSON.stringify(tool);
    if (sentSigs.has(sig)) return;
    sentSigs.add(sig);
    if (sentSigs.size > 200) sentSigs.delete(sentSigs.values().next().value);
    showWorkingStage(tool);
    chrome.runtime
      .sendMessage({ type: "tool", tool })
      .then((resp) => {
        if (resp?.ok && resp.id && tool.name === "run_command") {
          mountRunningTerminal(resp.id, tool.arguments?.command || "command");
        }
      })
      .catch(() => {});
  }

  /**
   * Mount a terminal for a command that is still running, so its Stop
   * button is reachable before the 120s timeout. The result path later
   * completes this same widget instead of mounting a second one.
   */
  function mountRunningTerminal(id, command) {
    const root = ensureDock()?.timeline;
    if (!root) return;
    const terminal = new C.ACBTerminal(command, id);
    const entry = { terminal, output: "" };
    terminal.onAction((action) => {
      if (action === "insert" && entry.output) insertIntoComposer(entry.output);
    });
    terminal.onStop(() => {
      chrome.runtime.sendMessage({ type: "cancel-tool", id }).catch(() => {});
    });
    terminal.mount(root);
    runningTerms.set(id, entry);
  }

  let lastScanned = "";
  const scan = () => {
    let source = null;
    const sel = MESSAGES[host];
    if (sel) {
      const nodes = document.querySelectorAll(sel);
      if (nodes.length > 0) source = nodes[nodes.length - 1].textContent;
    }
    // No `document.body.innerText` fallback: it forced a whole-document
    // layout on every scan, and it read the composer — so an auto-inserted
    // result or a pasted manifest re-triggered itself before being sent.
    if (source == null) {
      console.debug("[ACB] no message node matched for", host, "—", sel);
      return;
    }
    if (source === lastScanned) return;
    lastScanned = source;
    // Tagged and fenced JSON blocks first; then one-line calls on what's left.
    const { tools, rest } = S.extractTools(source);
    for (const tool of tools) sendTool(tool);
    for (const line of rest.split("\n")) {
      sendTool(S.parseToolLine(line.trim()));
    }
  };

  // Mutations from our own dock would otherwise schedule a scan on every
  // entry mount and every composer insert.
  const OWN = "#acb-dock, .acb-handoff-overlay, .acb-toast";

  const observer = new MutationObserver((records) => {
    const relevant = records.some((r) => {
      const node = r.target.nodeType === 1 ? r.target : r.target.parentElement;
      return !node || !node.closest(OWN);
    });
    if (!relevant) return;
    clearTimeout(observer._t);
    observer._t = setTimeout(scan, 900);
  });
  observer.observe(document.documentElement, { childList: true, subtree: true });

  // ── Dock (panel + timeline) ─────────────────────────────────────
  let dock = null;

  function ensureDock() {
    if ((!dock || !dock.el.isConnected) && C) {
      dock = new C.ACBDock();
      dock.mount(document.body);
      dock.setStatus("connecting");
      dock.onHistory(showHistory);
      // Pushed status only arrives on change; a fresh dock must ask for
      // the current state or it shows "connecting…" forever.
      chrome.runtime
        .sendMessage({ type: "get-status" })
        .then((s) => dock?.setStatus(s?.connected ? "connected" : "disconnected"))
        .catch(() => {});
    }
    return dock;
  }

  /** Open the dock's History view — past tool calls from the background's
   *  persistent log, the extension's counterpart of the desktop audit trail. */
  function showHistory() {
    const d = ensureDock();
    if (!d) return;
    d.panel.querySelector(".acb-history")?.remove();
    chrome.runtime
      .sendMessage({ type: "get-history" })
      .then((entries) => new C.ACBHistoryPanel(entries).mount(d.panel))
      .catch(() => {});
  }

  // ── Global close handler (event delegation — always works) ──────
  document.addEventListener("click", (e) => {
    const closeBtn = e.target.closest(".acb-close");
    if (!closeBtn) return;
    e.stopPropagation();
    e.preventDefault();
    // The History panel is a full-dock overlay, not a timeline widget,
    // but it closes through the same path.
    const widget = closeBtn.closest(".acb-widget, .acb-history");
    if (widget) {
      widget.setAttribute("data-state", "dismissed");
      setTimeout(() => widget.remove(), 200);
    }
  }, true);

  // ── Stage (dock footer line) ────────────────────────────────────
  const showWorkingStage = (tool) => ensureDock()?.setStage(S.stageLabel(tool), "working");
  const markStageDone = () => ensureDock()?.setStage("Finished ✓", "done");
  const markStageInserted = () => ensureDock()?.setStage("Inserted ✓", "done");
  const markStageFailed = () => ensureDock()?.setStage("Failed ✗", "error");
  const markStageAwait = () => ensureDock()?.setStage("Awaiting approval…", "working");

  // ── Widget rendering ────────────────────────────────────────────
  const TARGET_LABEL = {
    claudeai: "Continue with Claude.ai",
    gemini: "Continue with Gemini",
    grok: "Continue with Grok",
    chatgpt: "Continue with ChatGPT",
  };

  function showHandoffCard(h) {
    const label = TARGET_LABEL[h.target] || TARGET_LABEL.chatgpt;
    const card = new C.ACBHandoffCard(h, label);
    card.onAction((action) => {
      if (action === "continue") {
        const inserted = insertIntoComposer(HANDOFF_PROMPT(h));
        if (inserted && h.auto) setTimeout(submitComposer, 400);
      }
    });
    card.mount(document.body);
    if (h.auto) {
      const inserted = insertIntoComposer(HANDOFF_PROMPT(h));
      if (inserted) setTimeout(submitComposer, 400);
      card.destroy();
    }
  }

  function showToolWidget(msg) {
    const root = ensureDock()?.timeline;
    if (!root) return;

    // Handle v2 tool_result format
    if (msg.type === "tool_result") {
      const status = msg.status;
      const result = msg.result || {};
      const error = msg.error || {};
      const meta = msg.meta || {};

      if (status === "pending") {
        markStageAwait();
        // A live run_command terminal already shows the command; only the
        // note needs to change. Other tools still get a card.
        const live = runningTerms.get(msg.id);
        if (live) {
          live.terminal.setNote("Awaiting approval…");
          return;
        }
        const toolObj = { name: meta.tool || "tool", arguments: meta };
        new C.ACBToolCard(toolObj, msg.id).mount(root);
        return;
      }

      if (status === "denied" || status === "timeout" || status === "error") {
        markStageFailed();
        // Complete the live terminal in place — a second widget for the
        // same command would just duplicate its header.
        const live = runningTerms.get(msg.id);
        if (live) {
          runningTerms.delete(msg.id);
          live.output = error.message || (status === "error" ? "Unknown error" : status);
          live.terminal.setOutput(live.output);
          live.terminal.finish(false, null);
          return;
        }
        const resultBlock = new C.ACBResultBlock(
          { ok: false, output: error.message || (status === "error" ? "Unknown error" : status) },
          meta.tool || "tool",
          { detail: meta.path || meta.command || meta.detail || "", errorCode: error.code || "" },
        );
        resultBlock.mount(root);
        return;
      }

      // status === "success"
      if (meta.tool === "run_command") {
        const output = result.output || "";
        const live = runningTerms.get(msg.id);
        if (live) {
          runningTerms.delete(msg.id);
          live.output = output;
          live.terminal.setOutput(output);
          live.terminal.finish(true, meta.duration_ms ? `${meta.duration_ms}ms` : null);
        } else {
          // No live terminal (e.g. this tab reloaded mid-command) — mount
          // the finished widget the way it always rendered.
          const terminal = new C.ACBTerminal(
            meta.command || "command",
            msg.id,
          );
          terminal.setOutput(output);
          terminal.finish(true, meta.duration_ms ? `${meta.duration_ms}ms` : null);
          terminal.onAction((action) => {
            if (action === "insert") insertIntoComposer(output);
          });
          terminal.mount(root);
        }
        // The web AI only sees what reaches the composer, so successful
        // command output is pasted automatically — the Insert button can
        // re-add it (or add it again after the user edits it away).
        if (output) insertIntoComposer(output);
        markStageInserted();
        return;
      }

      // Read-only results go straight back into the chat.
      if (S.isAutoInsert(meta.tool) && result.output) {
        insertIntoComposer(result.output);
        markStageInserted();
        return;
      }

      const resultBlock = new C.ACBResultBlock(
        { ok: true, output: result.output },
        meta.tool || "tool",
        { detail: meta.path || meta.command || meta.detail || "" },
      );
      resultBlock.onAction((action) => {
        if (action === "insert" && result.output) {
          insertIntoComposer(result.output);
        }
      });
      resultBlock.mount(root);
      markStageDone();
      return;
    }

    // Legacy v1: tool-result format
    const r = msg.result;
    const t = S.normalizeTool(msg.tool) || { name: "tool", args: {} };

    if (r.pending) {
      // Informational only — approval resolves in the desktop app.
      markStageAwait();
      new C.ACBToolCard(msg.tool || {}, msg.id).mount(root);
      return;
    }

    if (t.name === "run_command") {
      const terminal = new C.ACBTerminal(t.args.command || "command", msg.id);
      const output = r.ok ? (r.output ?? "") : (r.error ?? "");
      terminal.setOutput(output);
      terminal.finish(r.ok, null);
      terminal.onAction((action) => {
        if (action === "insert") insertIntoComposer(output);
      });
      terminal.mount(root);
      // Mirror the v2 path: paste successful output for the web AI.
      if (r.ok && output) insertIntoComposer(output);
      r.ok ? markStageInserted() : markStageFailed();
      return;
    }

    // Read-only results go straight back into the chat.
    if (r.ok && S.isAutoInsert(t.name) && r.output) {
      insertIntoComposer(r.output);
      markStageInserted();
      return;
    }

    const result = new C.ACBResultBlock(
      { ok: r.ok, output: r.ok ? r.output : r.error },
      t.name,
    );
    result.onAction((action) => {
      if (action === "insert") {
        const text = r.ok ? r.output : r.error;
        if (text) insertIntoComposer(text);
      }
    });
    result.mount(root);
    r.ok ? markStageDone() : markStageFailed();
  }

  // ── Message listener ────────────────────────────────────────────
  chrome.runtime.onMessage.addListener((msg) => {
    if (msg.type === "handoff" && msg.payload) showHandoffCard(msg.payload);
    if (msg.type === "handoff-error") {
      const el = document.createElement("div");
      el.className = "acb-toast";
      el.innerHTML = `<span class="acb-status-dot"></span><span class="acb-status-text">Handoff failed: ${C.escapeHtml(msg.error || "unknown error")}</span>`;
      document.body.appendChild(el);
      setTimeout(() => el.remove(), 8000);
    }
    // Prime an already-open chat with the tool manifest (popup button).
    if (msg.type === "send-manifest") {
      const inserted = insertIntoComposer(S.promptToolSection());
      if (inserted) setTimeout(submitComposer, 400);
    }
    // v2: tool_result
    if (msg.type === "tool_result") showToolWidget(msg);
    // v2: cancel-ok — the core answered a Stop click. The terminal's
    // final state still comes from the tool_result that follows.
    if (msg.type === "cancel-ok" && msg.id) {
      const live = runningTerms.get(msg.id);
      if (live) live.terminal.setNote(msg.killed > 0 ? "Stopping…" : "Nothing to kill — awaiting approval");
    }
    // v1: tool-result (legacy)
    if (msg.type === "tool-result" && msg.result) showToolWidget(msg);
    if (msg.type === "status") {
      ensureDock()?.setStatus(msg.connected ? "connected" : "disconnected");
    }
  });
})();
