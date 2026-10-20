// The deployer page. Everything it shows comes from the deployer process
// on this computer: the views and fields from /api/schema, the values,
// prechecks, confirmations, and log from /api/state. The page never decides
// whether a job may run or needs confirming; the server does, with the same
// code the CLI uses.
"use strict";

(function () {
  const POLL_MS = 500;
  const ROOTS_POLL_MS = 4000;
  const SAVE_DELAY_MS = 250;
  const STORE_KEY = "omt-deployer-fields";
  const TOKEN_KEY = "omt-deployer-token";

  const INTROS = {
    "Connection": "How the deployer reaches the Raspberry Pi over SSH. Every step except SD card uses these settings.",
    "SD card": "Prepare a card already flashed with Alpine for Raspberry Pi for its first headless boot: the deployer writes the verified headless bootstrap overlay and a Wi-Fi configuration to its boot partition.",
    "Alpine": "Install Alpine in persistent sys mode on a factory Raspberry Pi image, with a hostname and new root and pi passwords. This erases the Pi's boot disk and reboots it.",
    "Deploy": "Upload, verify, install, and start the appliance on the Pi.",
    "Manage": "Check on, restart, rename, or re-secure an installed appliance.",
    "Wi-Fi": "Apply Wi-Fi settings on an installed appliance.",
    "Activity": "Progress and results of the jobs run in this session.",
    "About": "",
  };

  const HELP = {
    host: "Hostname or IP address of the Raspberry Pi.",
    user: "root on a factory Alpine image; pi once Alpine setup has run.",
    password: "Leave empty for an untouched factory Alpine image.",
    known_hosts: "An OpenSSH known_hosts file to check the Pi's host key against.",
    boot_directory: "The boot partition of the flashed card, as mounted on this computer.",
    sd_wifi_country: "Two-letter country code, such as US, GB, or DE.",
    remote_directory: "Where the appliance is installed on the Pi.",
    manage_hostname: "One DNS label of 1-63 characters.",
    alpine_hostname: "One DNS label of 1-63 characters.",
  };

  const COUNTRIES = ["US", "CA", "GB", "IE", "DE", "FR", "NL", "BE", "ES", "IT", "PT", "CH", "AT",
    "SE", "NO", "DK", "FI", "PL", "CZ", "AU", "NZ", "JP", "KR", "SG", "IN", "BR", "MX", "ZA"];

  const $ = (id) => document.getElementById(id);
  const el = (tag, attrs, ...children) => {
    const e = document.createElement(tag);
    for (const [k, v] of Object.entries(attrs || {})) {
      if (v === false || v === null || v === undefined) continue;
      if (k === "class") e.className = v;
      else if (k === "text") e.textContent = v;
      else if (k.startsWith("on")) e.addEventListener(k.slice(2), v);
      else e.setAttribute(k, v === true ? "" : v);
    }
    for (const c of children) if (c !== null && c !== undefined) e.append(c);
    return e;
  };
  const slug = (title) => title.toLowerCase().replace(/[^a-z0-9]+/g, "-");

  // ------------------------------------------------------------ session

  let token = "";
  {
    const m = /(?:^#|&)t=([0-9a-f]+)/.exec(location.hash);
    try {
      if (m) sessionStorage.setItem(TOKEN_KEY, m[1]);
      token = (m && m[1]) || sessionStorage.getItem(TOKEN_KEY) || "";
    } catch (_) {
      token = (m && m[1]) || "";
    }
    // The token leaves the address bar, so it is not copied by accident;
    // a view named beside it (#sd-card&t=…) stays.
    if (m) {
      const view = location.hash.slice(1).split("&").filter((p) => !p.startsWith("t="))[0];
      history.replaceState(null, "", location.pathname + "#" + (view || "connection"));
    }
  }

  function fatal(text) {
    const f = $("fatal");
    f.textContent = text;
    f.hidden = false;
  }

  let stopped = false;
  async function api(method, path, body) {
    const init = { method, headers: { "X-Deploy-Token": token }, cache: "no-store" };
    if (body !== undefined) {
      init.headers["Content-Type"] = "application/json";
      init.body = JSON.stringify(body);
    }
    let res;
    try {
      res = await fetch(path, init);
    } catch (_) {
      if (!stopped) setStatus("neutral", "Offline", "The deployer is not answering. Is it still running?");
      throw new Error("offline");
    }
    const data = await res.json().catch(() => ({}));
    if (res.status === 403) {
      fatal(data.error || "This page's session is not valid. Open the address the deployer printed.");
    }
    if (!res.ok) throw new Error(data.error || res.statusText);
    return data;
  }

  // ------------------------------------------------------------ storage

  function loadStored() {
    try {
      return JSON.parse(localStorage.getItem(STORE_KEY) || "{}") || {};
    } catch (_) {
      return {};
    }
  }

  function store(fields) {
    try {
      const keep = {};
      for (const [k, v] of Object.entries(fields)) if (typeof v === "string") keep[k] = v;
      localStorage.setItem(STORE_KEY, JSON.stringify(keep));
    } catch (_) {
      // Remembering is a convenience; nothing depends on it.
    }
  }

  // -------------------------------------------------------------- views

  let schema = null;
  let state = null;
  const inputs = {}; // field name -> [input elements]
  const toggles = {}; // slot name -> checkbox
  const buttons = {}; // slot name -> { button, help }
  const dirty = {}; // field name -> value not yet sent
  let saveTimer = 0;
  let currentView = "connection";

  function flush() {
    clearTimeout(saveTimer);
    const fields = Object.assign({}, dirty);
    for (const k of Object.keys(dirty)) delete dirty[k];
    if (Object.keys(fields).length === 0) return Promise.resolve();
    return api("POST", "/api/fields", { fields }).then(render, (e) => setStatus("danger", "Not saved", e.message));
  }

  function edited(field, value, source) {
    dirty[field] = value;
    // Rows sharing a value (the Wi-Fi SSID and password appear on three
    // views) follow each other.
    for (const i of inputs[field] || []) if (i !== source) i.value = value;
    clearTimeout(saveTimer);
    saveTimer = setTimeout(flush, SAVE_DELAY_MS);
    checkConfirm(field);
    if (field === "boot_directory") checkBootDirectory();
  }

  const CONFIRMS = {
    os_root_confirm: "os_root_password",
    os_pi_confirm: "os_pi_password",
    web_confirm: "web_password",
    manage_web_confirm: "manage_web_password",
  };
  const matchNotes = {};

  function checkConfirm(field) {
    for (const [confirm, original] of Object.entries(CONFIRMS)) {
      if (field !== confirm && field !== original) continue;
      const note = matchNotes[confirm];
      const a = (inputs[original] || [])[0];
      const b = (inputs[confirm] || [])[0];
      if (!note || !a || !b) continue;
      if (b.value === "") {
        note.textContent = "";
        note.className = "field-help";
      } else if (a.value === b.value) {
        note.textContent = "Passwords match.";
        note.className = "field-help match-ok";
      } else {
        note.textContent = "Does not match yet.";
        note.className = "field-help match-bad";
      }
    }
  }

  function textField(s, secret) {
    const id = "f-" + s.name;
    const help = HELP[s.name];
    const input = el("input", {
      id,
      type: secret ? "password" : "text",
      autocomplete: secret ? "new-password" : "off",
      spellcheck: "false",
      "aria-describedby": help ? id + "-help" : null,
      oninput: (e) => {
        if (s.name === "sd_wifi_country") e.target.value = e.target.value.toUpperCase();
        edited(s.field, e.target.value, e.target);
      },
    });
    (inputs[s.field] = inputs[s.field] || []).push(input);
    const row = el("div", { class: "input-row" }, input);
    if (secret) {
      const show = el("button", {
        type: "button",
        class: "button button-secondary",
        "aria-pressed": "false",
        "aria-label": "Show " + s.label,
        text: "Show",
        onclick: () => {
          const on = input.type === "password";
          input.type = on ? "text" : "password";
          show.textContent = on ? "Hide" : "Show";
          show.setAttribute("aria-pressed", String(on));
        },
      });
      row.append(show);
    }
    if (s.name === "boot_directory") {
      row.append(el("button", {
        type: "button", class: "button button-secondary", text: "Browse…",
        onclick: () => openPicker({ field: s.field, files: false, start: input.value }),
      }));
    }
    if (s.name === "known_hosts") {
      row.append(el("button", {
        type: "button", class: "button button-secondary", text: "Browse…",
        onclick: () => openPicker({ field: s.field, files: true, start: parentOf(input.value) }),
      }));
    }
    if (s.name === "sd_wifi_country") {
      input.setAttribute("list", "countries");
      input.setAttribute("maxlength", "2");
      input.classList.add("short-input");
    }
    const wrap = el("div", { class: "field" }, el("label", { for: id, text: s.label }), row);
    if (help) wrap.append(el("p", { class: "field-help", id: id + "-help", text: help }));
    if (CONFIRMS[s.field]) {
      const note = el("p", { class: "field-help", "aria-live": "polite" });
      matchNotes[s.field] = note;
      wrap.append(note);
    }
    if (s.name === "boot_directory") wrap.append(bootPanel());
    return wrap;
  }

  function parentOf(path) {
    const i = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
    return i > 0 ? path.slice(0, i) : "";
  }

  // The SD card view's detected boot partitions and its verdict on the
  // chosen folder.
  const boot = { verdict: null, list: null, help: null };

  function bootPanel() {
    boot.verdict = el("p", { class: "field-help", "aria-live": "polite" });
    boot.list = el("div", { class: "chip-row", "aria-label": "Detected boot partitions" });
    boot.help = el("p", { class: "field-help" });
    const rescan = el("button", { type: "button", class: "button button-secondary", text: "Rescan", onclick: scanRoots });
    return el("div", { class: "boot-panel" }, boot.verdict,
      el("p", { class: "eyebrow", text: "Detected Alpine boot partitions" }),
      el("div", { class: "chip-row" }, boot.list, rescan), boot.help);
  }

  let bootCheck = 0;
  function checkBootDirectory() {
    const value = ((inputs.boot_directory || [])[0] || {}).value || "";
    const mine = ++bootCheck;
    if (!boot.verdict) return;
    if (value.trim() === "") {
      boot.verdict.textContent = "";
      return;
    }
    api("GET", "/api/fs/list?path=" + encodeURIComponent(value)).then((l) => {
      if (mine !== bootCheck) return;
      boot.verdict.textContent = l.alpine_boot
        ? "✓ This is an Alpine Raspberry Pi boot partition."
        : "This folder is not an Alpine Raspberry Pi boot partition.";
      boot.verdict.className = "field-help " + (l.alpine_boot ? "match-ok" : "match-bad");
    }, () => {
      if (mine !== bootCheck) return;
      boot.verdict.textContent = "This folder does not exist.";
      boot.verdict.className = "field-help match-bad";
    });
  }

  function scanRoots() {
    return api("GET", "/api/fs/roots").then((r) => {
      if (!boot.list) return r;
      const found = r.roots.filter((x) => x.alpine_boot);
      boot.list.replaceChildren(...found.map((x) => el("button", {
        type: "button", class: "button button-secondary chip", title: x.path,
        text: x.label + " — " + x.path,
        onclick: () => useFolder("boot_directory", x.path),
      })));
      boot.help.textContent = found.length
        ? "Choose one, or Browse… for another folder."
        : "None found. Insert the flashed card and let the desktop mount it, then Rescan — or Browse… to it.";
      return r;
    }, () => null);
  }

  function useFolder(field, path) {
    for (const i of inputs[field] || []) i.value = path;
    edited(field, path, null);
  }

  function toggleField(s) {
    const id = "f-" + s.name;
    const box = el("input", {
      id, type: "checkbox",
      onchange: (e) => api("POST", "/api/fields", { toggles: { [s.name]: e.target.checked } }).then(render, (err) => setStatus("danger", "Not saved", err.message)),
    });
    toggles[s.name] = box;
    return el("label", { class: "checkbox-label", for: id }, box, " " + s.label);
  }

  function actionButton(s) {
    const b = el("button", {
      type: "button",
      class: "button" + (s.confirm ? " button-danger" : ""),
      text: s.label,
      onclick: () => press(s.name),
    });
    const help = el("p", { class: "field-help blocked-reason", "aria-live": "polite" });
    buttons[s.name] = { button: b, help };
    return { b, help };
  }

  async function press(name) {
    await flush();
    try {
      render(await api("POST", "/api/press", { slot: name }));
    } catch (e) {
      setStatus("danger", "Not started", e.message);
    }
  }

  // Cards: each run of inputs ends at the button that uses them;
  // consecutive buttons with nothing between them share a row.
  const CARD_TITLES = {
    test: "SSH connection", prepare_sd: "Boot partition", alpine: "Alpine installation",
    deploy: "Appliance", status: "Appliance", restart: "Restart or reboot", apply_hostname: "Hostname",
    change_web_password: "Web GUI password", apply_wifi: "Wi-Fi network",
  };

  function buildView(v, i) {
    const section = el("section", { id: "view-" + slug(v.title), class: "view", "aria-labelledby": "h-" + i, hidden: true },
      el("h1", { id: "h-" + i, text: v.title }));
    if (INTROS[v.title]) section.append(el("p", { class: "page-intro", text: INTROS[v.title] }));
    let pending = [];
    let row = null;
    let card = null;
    for (const s of v.slots) {
      if (s.kind !== "button") {
        pending.push(s.kind === "toggle" ? toggleField(s) : textField(s, s.kind === "secret"));
        row = null;
        continue;
      }
      const { b, help } = actionButton(s);
      // An action that cannot be undone gets a danger-zone card of its own
      // rather than sharing one with harmless reads.
      if (row && pending.length === 0 && Boolean(s.confirm) === card.classList.contains("danger-zone")) {
        row.append(b);
        card.append(help);
        continue;
      }
      card = el("div", { class: "card" + (s.confirm ? " danger-zone" : "") },
        el("h2", { text: CARD_TITLES[s.name] || s.label }), ...pending);
      row = el("div", { class: "button-row" }, b);
      card.append(row, help);
      section.append(card);
      pending = [];
    }
    if (v.title === "Activity") section.append(activityCard());
    if (v.title === "About") section.append(aboutCards());
    return section;
  }

  // ----------------------------------------------------------- activity

  let logBox = null;
  let follow = true;
  let logEnd = 0;
  const LOG_KEEP = 2000;

  function activityCard() {
    logBox = el("pre", { class: "diagnostic-output activity-log", tabindex: "0", "aria-label": "Activity log" });
    logBox.addEventListener("scroll", () => {
      follow = logBox.scrollTop + logBox.clientHeight >= logBox.scrollHeight - 8;
    });
    const copy = el("button", {
      type: "button", class: "button button-secondary", text: "Copy log",
      onclick: () => navigator.clipboard.writeText(logBox.textContent).then(
        () => { copy.textContent = "Copied"; setTimeout(() => { copy.textContent = "Copy log"; }, 1500); },
        () => { copy.textContent = "Copy failed"; }),
    });
    const latest = el("button", {
      type: "button", class: "button button-secondary", text: "Jump to latest",
      onclick: () => { follow = true; logBox.scrollTop = logBox.scrollHeight; },
    });
    return el("div", { class: "card" }, el("h2", { text: "Log" }), logBox, el("div", { class: "button-row" }, copy, latest));
  }

  function appendLog(lines) {
    if (!logBox || lines.length === 0) return;
    const text = logBox.textContent + (logBox.textContent ? "\n" : "") + lines.join("\n");
    const all = text.split("\n");
    logBox.textContent = all.length > LOG_KEEP ? all.slice(all.length - LOG_KEEP).join("\n") : text;
    if (follow) logBox.scrollTop = logBox.scrollHeight;
  }

  function aboutCards() {
    const summary = el("pre", { class: "legal-text" });
    const license = el("pre", { class: "legal-text" });
    const notices = el("pre", { class: "legal-text" });
    api("GET", "/api/about").then((a) => {
      summary.textContent = a.summary;
      license.textContent = a.license;
      notices.textContent = a.notices;
    }, () => {});
    const frag = document.createDocumentFragment();
    frag.append(
      el("div", { class: "card" }, el("h2", { text: "This deployer" }), summary),
      el("div", { class: "card legal-card" }, el("h2", { text: "License" }), license),
      el("div", { class: "card legal-card" }, el("h2", { text: "Third-party notices" }), notices));
    return frag;
  }

  // ---------------------------------------------------------- navigation

  function showView(name) {
    if (!schema) return;
    const known = schema.views.map((v) => slug(v.title));
    if (!known.includes(name)) name = "connection";
    currentView = name;
    for (const v of schema.views) {
      const s = slug(v.title);
      $("view-" + s).hidden = s !== name;
      const link = $("nav-" + s);
      if (s === name) link.setAttribute("aria-current", "page");
      else link.removeAttribute("aria-current");
    }
    if (name === "sd-card") scanRoots();
    if (name === "activity" && logBox && follow) logBox.scrollTop = logBox.scrollHeight;
  }

  window.addEventListener("hashchange", () => showView(location.hash.slice(1)));

  // -------------------------------------------------------------- state

  function setStatus(tone, token_, text) {
    const t = $("state-token");
    t.className = "state-token banner-" + tone;
    t.textContent = token_;
    $("status-text").textContent = text;
  }

  let wasBusy = false;
  let confirmOpen = null;

  function render(s) {
    if (!s || s.busy === undefined) return;
    state = s;
    // Values: an input being edited, or with an edit not yet sent, keeps
    // what the operator typed.
    for (const [field, value] of Object.entries(s.fields)) {
      for (const i of inputs[field] || []) {
        if (field in dirty || document.activeElement === i) continue;
        if (typeof value === "string") {
          if (i.value !== value) i.value = value;
        } else {
          i.placeholder = value.set ? "Set — type to replace" : "";
          if (!value.set) i.value = "";
        }
      }
    }
    for (const [name, on] of Object.entries(s.toggles)) {
      if (toggles[name] && document.activeElement !== toggles[name]) toggles[name].checked = on;
    }
    for (const [name, problem] of Object.entries(s.blocked)) {
      const b = buttons[name];
      if (!b) continue;
      b.button.disabled = s.busy || problem !== null;
      b.help.textContent = s.busy ? "" : problem || "";
    }
    for (const input of document.querySelectorAll("#views input, #views select")) {
      input.disabled = s.busy;
    }
    const nonSecret = {};
    for (const [k, v] of Object.entries(s.fields)) if (typeof v === "string") nonSecret[k] = v;
    store(nonSecret);

    // Only the poll carries the log; an action's answer leaves it out.
    if (s.log !== undefined) {
      if (s.log_start > logEnd && logBox && logBox.textContent) appendLog(["… earlier lines dropped …"]);
      appendLog(s.log);
      logEnd = s.log_end;
    }

    $("cancel-job").hidden = !s.busy;
    $("cancel-job").disabled = s.cancelling;
    if (s.busy) setStatus("warning", s.cancelling ? "Cancelling" : "Running", s.status);
    else if (s.status.startsWith("Failed")) setStatus("danger", "Failed", s.status);
    else if (s.status.startsWith("Finished")) setStatus("success", "Done", s.status);
    else setStatus("neutral", "Ready", s.status);
    if (s.busy && !wasBusy) {
      // Progress belongs in front of the operator, not behind a tab.
      location.hash = "activity";
      // The server holds what was typed; the page need not.
      for (const [field, list] of Object.entries(inputs)) {
        if (s.fields[field] && typeof s.fields[field] !== "string") for (const i of list) i.value = "";
      }
      for (const f of Object.keys(CONFIRMS)) checkConfirm(f);
    }
    wasBusy = s.busy;

    const dlg = $("confirm-dialog");
    if (s.confirm && confirmOpen !== s.confirm) {
      confirmOpen = s.confirm;
      $("confirm-text").textContent = s.confirm;
      if (!dlg.open) dlg.showModal();
      $("confirm-no").focus();
    } else if (!s.confirm && dlg.open) {
      confirmOpen = null;
      dlg.close();
    }
    if (s.quit) stop();
  }

  function stop() {
    stopped = true;
    clearInterval(pollTimer);
    clearInterval(rootsTimer);
    $("stopped").hidden = false;
    setStatus("neutral", "Stopped", "The deployer has stopped.");
  }

  function answer(accept) {
    confirmOpen = null;
    const dlg = $("confirm-dialog");
    if (dlg.open) dlg.close();
    api("POST", "/api/confirm", { accept }).then(render, (e) => setStatus("danger", "Error", e.message));
  }

  $("confirm-yes").addEventListener("click", () => answer(true));
  $("confirm-no").addEventListener("click", () => answer(false));
  $("confirm-dialog").addEventListener("cancel", (e) => { e.preventDefault(); answer(false); });
  $("cancel-job").addEventListener("click", () => api("POST", "/api/cancel", {}).then(render, () => {}));
  $("quit").addEventListener("click", () => api("POST", "/api/quit", {}).then((s) => {
    render(s);
    // The server stops once it has answered; there is nothing to poll.
    if (s.quit) stop();
  }, () => {}));

  let polling = false;
  async function poll() {
    if (polling || stopped) return;
    polling = true;
    try {
      render(await api("GET", "/api/state?since=" + logEnd));
    } catch (_) {
      // Shown by api(); the next tick retries.
    } finally {
      polling = false;
    }
  }

  // ------------------------------------------------------------- picker

  const picker = { field: null, files: false, path: "", opener: null };

  function openPicker(opts) {
    picker.field = opts.field;
    picker.files = opts.files;
    picker.opener = document.activeElement;
    $("picker-title").textContent = opts.files ? "Choose a file" : "Choose a folder";
    $("picker-use").hidden = opts.files;
    $("picker").showModal();
    api("GET", "/api/fs/roots").then((r) => {
      $("picker-roots").replaceChildren(...r.roots.map((x) => el("button", {
        type: "button", class: "button button-secondary chip" + (x.alpine_boot ? " chip-boot" : ""),
        title: x.path, text: x.label, onclick: () => browse(x.path),
      })));
      const firstBoot = r.roots.find((x) => x.alpine_boot);
      browse(opts.start || (!opts.files && firstBoot ? firstBoot.path : ""));
    }, () => browse(opts.start || ""));
  }

  function browse(path) {
    const q = "/api/fs/list?path=" + encodeURIComponent(path) +
      (picker.files ? "&files=1" : "") + ($("picker-hidden").checked ? "&hidden=1" : "");
    api("GET", q).then((l) => {
      picker.path = l.path;
      $("picker-path").value = l.path;
      $("picker-up").disabled = !l.parent || l.parent === l.path;
      $("picker-up").dataset.parent = l.parent || "";
      $("picker-current").textContent = l.alpine_boot
        ? "✓ This folder is an Alpine Raspberry Pi boot partition."
        : (l.truncated ? "Showing the first " + l.entries.length + " items." : "");
      $("picker-current").className = "picker-current" + (l.alpine_boot ? " match-ok" : "");
      const items = l.entries.map((e) => {
        const open = el("button", {
          type: "button", class: "picker-item" + (e.dir ? " is-dir" : " is-file"),
          text: e.name,
          onclick: () => (e.dir ? browse(e.path) : pick(e.path)),
        });
        const li = el("li", {}, open);
        if (e.alpine_boot) {
          li.append(el("span", { class: "badge banner-success", text: "Alpine boot" }));
          if (!picker.files) li.append(el("button", { type: "button", class: "button chip", text: "Use", onclick: () => pick(e.path) }));
        }
        return li;
      });
      if (items.length === 0) items.push(el("li", { class: "picker-empty", text: picker.files ? "Nothing here." : "No folders here." }));
      $("picker-list").replaceChildren(...items);
    }, (e) => {
      $("picker-current").textContent = e.message;
      $("picker-current").className = "picker-current match-bad";
    });
  }

  function pick(path) {
    useFolder(picker.field, path);
    closePicker();
  }

  function closePicker() {
    $("picker").close();
    if (picker.opener && picker.opener.focus) picker.opener.focus();
  }

  $("picker-path-form").addEventListener("submit", (e) => { e.preventDefault(); browse($("picker-path").value); });
  $("picker-up").addEventListener("click", (e) => browse(e.currentTarget.dataset.parent));
  $("picker-hidden").addEventListener("change", () => browse(picker.path));
  $("picker-use").addEventListener("click", () => pick(picker.path));
  $("picker-cancel").addEventListener("click", closePicker);

  // --------------------------------------------------------------- start

  let pollTimer = 0;
  let rootsTimer = 0;

  async function start() {
    if (!token) {
      fatal("This page needs the address the deployer printed when it started (it ends in #t=…).");
      return;
    }
    try {
      schema = await api("GET", "/api/schema");
    } catch (e) {
      if (e.message !== "offline") fatal(e.message);
      return;
    }
    const nav = $("nav");
    const views = $("views");
    schema.views.forEach((v, i) => {
      nav.append(el("a", { id: "nav-" + slug(v.title), href: "#" + slug(v.title), text: v.title }));
      views.append(buildView(v, i));
    });
    document.body.append(el("datalist", { id: "countries" }, ...COUNTRIES.map((c) => el("option", { value: c }))));

    const first = await api("GET", "/api/state?since=0").catch(() => null);
    // A fresh deployer starts from its defaults; bring back what this
    // browser last used, once per session.
    let restoredKey = "omt-deployer-restored-" + token;
    let restored = false;
    try { restored = sessionStorage.getItem(restoredKey) === "1"; } catch (_) { /* ignore */ }
    if (first && !first.busy && !restored) {
      const saved = loadStored();
      const fields = {};
      for (const [k, v] of Object.entries(saved)) {
        if (typeof first.fields[k] === "string" && typeof v === "string" && v !== first.fields[k]) fields[k] = v;
      }
      if (Object.keys(fields).length) await api("POST", "/api/fields", { fields }).catch(() => null);
      try { sessionStorage.setItem(restoredKey, "1"); } catch (_) { /* ignore */ }
    }
    showView(location.hash.slice(1) || "connection");
    await poll();
    for (const f of Object.keys(CONFIRMS)) checkConfirm(f);
    checkBootDirectory();
    pollTimer = setInterval(poll, POLL_MS);
    rootsTimer = setInterval(() => {
      if (currentView === "sd-card" && !document.hidden) scanRoots();
    }, ROOTS_POLL_MS);
  }

  start();
})();
