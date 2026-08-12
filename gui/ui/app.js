"use strict";

// The specification text is the single source of truth. Form controls do not
// keep a model of their own: each one asks the backend to edit the document
// and then everything re-reads it. That is slower than mirroring the state in
// the page, and it is the reason the file keeps its comments and cannot drift
// out of step with what will actually be written to a card.

const invoke = window.__TAURI__.core.invoke;

const state = {
  text: "",
  path: "",
  baseDir: ".",
  valid: false,
};

// Deliberately incomplete: it stops at the one decision nobody else can
// make. The validator then says "there would be no way to log in", which is
// the next thing to do rather than a complaint about a placeholder.
const TEMPLATE = `# Written by the rpi-provision window. Safe to commit: secrets are
# declared as a source, never as a literal.

[meta]
schema_version = 1

[system]
hostname = "pi-01"

[user]
name = "engineer"
# Paste a public key — the contents of ~/.ssh/id_ed25519.pub — or declare a
# password hash: password_hash = { env = "RPI_PASSWORD_HASH" }
authorized_keys = []
`;

const $ = (id) => document.getElementById(id);
const controls = () => document.querySelectorAll("[data-key]");

// ------------------------------------------------------------------ helpers

function directoryOf(path) {
  const cut = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  return cut > 0 ? path.slice(0, cut) : ".";
}

function secrets() {
  const collected = {};
  for (const row of document.querySelectorAll(".secret-row")) {
    const name = row.querySelector(".secret-name").value.trim();
    const value = row.querySelector(".secret-value").value;
    if (name) collected[name] = value;
  }
  return collected;
}

function specInput() {
  return { text: state.text, base_dir: state.baseDir, secrets: secrets() };
}

function show(text, tab) {
  $("output").textContent = text;
  if (tab !== false) selectTab("output");
}

function report(error) {
  show(String(error));
}

function selectTab(name) {
  for (const tab of document.querySelectorAll(".tab")) {
    tab.classList.toggle("active", tab.dataset.panel === name);
  }
  for (const panel of document.querySelectorAll(".panel")) {
    panel.classList.toggle("active", panel.id === `panel-${name}`);
  }
}

function setPill(kind, text) {
  const pill = $("state");
  pill.className = `pill pill-${kind}`;
  pill.textContent = text;
}

// ------------------------------------------------------------- the document

async function setText(text, { fromEditor = false } = {}) {
  state.text = text;
  if (!fromEditor) $("toml").value = text;
  await refresh();
}

async function refresh() {
  await Promise.all([refreshStatus(), refreshForm()]);
}

async function refreshStatus() {
  if (!state.text.trim()) {
    state.valid = false;
    setPill("idle", "no specification");
    $("status").textContent = "Waiting for a specification.";
    return;
  }
  const result = await invoke("validate", { input: specInput() });
  state.valid = result.ok;

  if (!result.ok) {
    setPill("bad", "invalid");
    $("status").innerHTML = "";
    const problem = document.createElement("pre");
    problem.className = "problem";
    problem.textContent = result.error;
    $("status").append(problem);
    return;
  }

  setPill(result.warnings.length ? "warn" : "good", result.warnings.length ? "warnings" : "valid");
  const s = result.summary;
  const rows = [
    ["host name", s.hostname],
    ["account", s.user],
    ["target", s.target],
    ["ssh", s.ssh],
    ["network", s.network],
    ["buses", s.hardware],
    ["transfers", `${s.files} file(s), ${s.run} command(s)`],
    ["digest", s.digest.slice(0, 16) + "…"],
  ];
  $("status").innerHTML = "";
  const table = document.createElement("dl");
  for (const [key, value] of rows) {
    const term = document.createElement("dt");
    term.textContent = key;
    const definition = document.createElement("dd");
    definition.textContent = value;
    table.append(term, definition);
  }
  $("status").append(table);
  for (const warning of result.warnings) {
    const note = document.createElement("p");
    note.className = "warning";
    note.textContent = warning;
    $("status").append(note);
  }
}

async function refreshForm() {
  const keys = [...controls()].map((control) => control.dataset.key);
  let values;
  try {
    values = await invoke("get_values", { text: state.text || "\n", paths: keys });
  } catch {
    // The document does not parse; leave the controls as they are rather
    // than blanking a form the operator is in the middle of using.
    return;
  }
  for (const control of controls()) {
    const value = values[control.dataset.key];
    if (control.type === "checkbox") {
      control.checked = value === true;
    } else if (control.dataset.list) {
      control.value = Array.isArray(value) ? value.join(", ") : "";
    } else {
      control.value = value === null || value === undefined ? "" : String(value);
    }
  }
}

async function onControlChanged(control) {
  let value;
  if (control.type === "checkbox") {
    value = control.checked;
  } else if (control.dataset.list) {
    const items = control.value.split(",").map((item) => item.trim()).filter(Boolean);
    value = items.length ? items : "";
  } else if (control.dataset.number) {
    value = control.value === "" ? "" : Number.parseInt(control.value, 10);
    if (Number.isNaN(value)) return;
  } else {
    value = control.value.trim();
  }

  try {
    const text = await invoke("set_value", {
      text: state.text || "\n",
      path: control.dataset.key,
      value,
    });
    await setText(text);
  } catch (error) {
    report(error);
  }
}

// ------------------------------------------------------------------ actions

async function withCard(action) {
  const boot = $("boot").value.trim();
  if (!boot) {
    show("Choose a boot partition first — press Detect, or type the path.");
    return;
  }
  try {
    await action(boot);
  } catch (error) {
    report(error);
  }
}

function renderPreview(preview) {
  const lines = [];
  for (const change of preview.changes) {
    if (change.kind === "unchanged" || change.kind === "absent") continue;
    lines.push(`${change.kind.padStart(9)}  ${change.path}`);
    if (change.diff) {
      for (const line of change.diff.split("\n")) {
        if (line) lines.push(`           ${line}`);
      }
    }
  }
  lines.push("");
  lines.push(
    `${preview.created} to create, ${preview.updated} to update, ` +
      `${preview.unchanged} unchanged, ${preview.deleted} to delete`,
  );
  for (const conflict of preview.conflicts) {
    lines.push(`warning: ${conflict} is another first-boot mechanism and would conflict`);
  }
  return lines.join("\n");
}

function confirm(title, body) {
  return new Promise((resolve) => {
    $("confirmTitle").textContent = title;
    $("confirmBody").textContent = body;
    $("confirm").hidden = false;
    const close = (answer) => {
      $("confirm").hidden = true;
      $("confirmYes").removeEventListener("click", yes);
      $("confirmNo").removeEventListener("click", no);
      resolve(answer);
    };
    const yes = () => close(true);
    const no = () => close(false);
    $("confirmYes").addEventListener("click", yes);
    $("confirmNo").addEventListener("click", no);
  });
}

// --------------------------------------------------------------- the wiring

function addSecretRow(name = "", value = "") {
  const row = document.createElement("div");
  row.className = "secret-row";

  const key = document.createElement("input");
  key.className = "secret-name";
  key.type = "text";
  key.placeholder = "RPI_PASSWORD_HASH";
  key.value = name;

  const secret = document.createElement("input");
  secret.className = "secret-value";
  secret.type = "password";
  secret.placeholder = "value";
  secret.value = value;

  const remove = document.createElement("button");
  remove.type = "button";
  remove.className = "subtle";
  remove.textContent = "×";
  remove.title = "Remove";
  remove.addEventListener("click", () => {
    row.remove();
    refreshStatus();
  });

  for (const field of [key, secret]) {
    field.addEventListener("change", refreshStatus);
  }
  row.append(key, secret, remove);
  $("secrets").append(row);
}

function wire() {
  invoke("generator").then((text) => {
    $("generator").textContent = text;
  });

  for (const tab of document.querySelectorAll(".tab")) {
    tab.addEventListener("click", () => selectTab(tab.dataset.panel));
  }

  for (const control of controls()) {
    control.addEventListener("change", () => onControlChanged(control));
  }

  $("toml").addEventListener("input", () => {
    state.text = $("toml").value;
  });
  $("toml").addEventListener("change", () => setText($("toml").value, { fromEditor: true }));
  $("toml").addEventListener("blur", () => setText($("toml").value, { fromEditor: true }));

  $("detect").addEventListener("click", async () => {
    const cards = await invoke("detect_cards");
    const list = $("cards");
    list.innerHTML = "";
    if (!cards.length) {
      const empty = document.createElement("li");
      empty.className = "muted";
      empty.textContent = "None found. Insert the card, or type the path.";
      list.append(empty);
      return;
    }
    for (const card of cards) {
      const item = document.createElement("li");
      const button = document.createElement("button");
      button.type = "button";
      button.className = "subtle";
      button.textContent = `${card.path} — ${card.model}`;
      button.addEventListener("click", () => {
        $("boot").value = card.path;
      });
      item.append(button);
      list.append(item);
    }
  });

  $("open").addEventListener("click", async () => {
    const path = $("specPath").value.trim();
    if (!path) return;
    try {
      const text = await invoke("read_spec", { path });
      state.path = path;
      state.baseDir = directoryOf(path);
      await setText(text);
      show(`Opened ${path}`, false);
    } catch (error) {
      report(error);
    }
  });

  $("save").addEventListener("click", async () => {
    const path = $("specPath").value.trim();
    if (!path) {
      show("Give the file a path first.");
      return;
    }
    try {
      await invoke("write_spec", { path, text: state.text });
      state.path = path;
      state.baseDir = directoryOf(path);
      show(`Saved ${path}`);
    } catch (error) {
      report(error);
    }
  });

  $("template").addEventListener("click", async () => {
    await setText(TEMPLATE);
    selectTab("form");
  });

  $("addSecret").addEventListener("click", () => addSecretRow());

  $("preview").addEventListener("click", () =>
    withCard(async (boot) => {
      const preview = await invoke("preview", { input: { ...specInput(), boot } });
      show(renderPreview(preview));
    }),
  );

  $("apply").addEventListener("click", () =>
    withCard(async (boot) => {
      if (!state.valid) {
        show("The specification is not valid yet; the Status panel says why.");
        return;
      }
      const preview = await invoke("preview", { input: { ...specInput(), boot } });
      show(renderPreview(preview));
      const total = preview.created + preview.updated + preview.deleted;
      if (total === 0) {
        show(`${boot} is already up to date.`);
        return;
      }
      const snapshotInto = $("backupFirst").checked ? $("snapshotDir").value.trim() : "";
      if ($("backupFirst").checked && !snapshotInto) {
        show("Give the snapshot a directory, or clear the snapshot checkbox.");
        return;
      }
      const agreed = await confirm(
        `Write ${total} change(s) to ${boot}?`,
        preview.conflicts.length
          ? `The card also carries ${preview.conflicts.join(", ")}, which is another ` +
              "first-boot mechanism. Applying on top of it leaves the order undefined."
          : "The change set is in the Output tab.",
      );
      if (!agreed) return;
      const summary = await invoke("apply_spec", {
        input: { ...specInput(), boot, backup_into: snapshotInto },
      });
      show(`${renderPreview(preview)}\n\n${summary}`);
    }),
  );

  $("revert").addEventListener("click", () =>
    withCard(async (boot) => {
      const agreed = await confirm(
        `Remove what apply added from ${boot}?`,
        "The managed block, the command line hooks and the payload directory go; " +
          "the rest of the card is left alone.",
      );
      if (!agreed) return;
      show(await invoke("revert_spec", { input: { ...specInput(), boot } }));
    }),
  );

  $("backup").addEventListener("click", () =>
    withCard(async (boot) => {
      const out = $("snapshotDir").value.trim();
      if (!out) {
        show("Give the snapshot a directory.");
        return;
      }
      show(`Snapshot of ${boot}: ${await invoke("backup_card", { boot, out })}`);
    }),
  );

  $("inspect").addEventListener("click", async () => {
    const from = $("snapshotDir").value.trim();
    if (!from) {
      show("Give the snapshot directory.");
      return;
    }
    try {
      show(await invoke("inspect_snapshot", { from }));
    } catch (error) {
      report(error);
    }
  });

  $("restore").addEventListener("click", () =>
    withCard(async (boot) => {
      const from = $("snapshotDir").value.trim();
      if (!from) {
        show("Give the snapshot directory.");
        return;
      }
      const described = await invoke("inspect_snapshot", { from });
      const agreed = await confirm(
        `Make ${boot} match the snapshot?`,
        `${described}. Files on the card that the snapshot does not have are deleted.`,
      );
      if (!agreed) return;
      show(await invoke("restore_card", { boot, from }));
    }),
  );

  addSecretRow("RPI_PASSWORD_HASH");
  refresh();
}

wire();
