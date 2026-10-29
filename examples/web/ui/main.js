// The wRPC wire encoding, framing and invocation are provided by the
// `@bytecodealliance/wrpc` client, served by the backend under `/wrpc`.
import { t, invoke as wrpcInvoke, fromWebStreams } from "/wrpc/index.js";

// `wasi:keyvalue/store`, as served by the backend. The types mirror the
// WIT in `crates/wasi-keyvalue/wit/deps/keyvalue/store.wit`; the codec
// walks them to encode arguments and decode results.
const STORE = "wasi:keyvalue/store@0.2.0-draft2";
const error = t.variant({ "no-such-store": null, "access-denied": null, other: t.string });

// open: func(identifier: string) -> result<bucket, error>
const OPEN = { params: [t.string], results: [t.result(t.own("bucket"), error)] };
// bucket.get: func(key: string) -> result<option<list<u8>>, error>
const GET = {
  params: [t.borrow("bucket"), t.string],
  results: [t.result(t.option(t.list(t.u8)), error)],
};
// bucket.set: func(key: string, value: list<u8>) -> result<_, error>
const SET = {
  params: [t.borrow("bucket"), t.string, t.list(t.u8)],
  results: [t.result(null, error)],
};

let conn;
let buckets = new Map();

// Invoke `func` on `instance` over a fresh WebTransport bidirectional
// stream. The framed transport maps one stream to one invocation; the
// client writes the request, streams any async parameters, and decodes
// the reply (and any async results) until all I/O is done.
async function invoke(instance, func, paramTypes, resultTypes, args) {
  if (!conn) throw "WebTransport not connected";
  const stream = await conn.createBidirectionalStream();
  const { results, done } = await wrpcInvoke(
    fromWebStreams(stream),
    instance,
    func,
    paramTypes,
    args,
    resultTypes,
  );
  await done;
  return results;
}

// Render a decoded `error` variant value (`{ kind, value? }`) as a string.
function formatError(err) {
  switch (err.kind) {
    case "no-such-store":
      return "no such store";
    case "access-denied":
      return "access denied";
    case "other":
      return err.value;
    default:
      return `unknown error \`${err.kind}\``;
  }
}

function uuidString(v) {
  return v.reduce((s, x) => {
    const hex = x.toString(16).padStart(2, "0");
    switch (s.length) {
      case 8:
      case 13:
      case 18:
      case 23:
        return s + "-" + hex;
      default:
        return s + hex;
    }
  }, "");
}

async function getBucket() {
  if (!conn) throw "WebTransport not connected";

  const obj = getFormValues("#settings");
  let identifier;
  let proto = obj.proto;
  if (proto === "mem") {
    identifier = "";
  } else if (proto === "redis") {
    const url = obj["redis-url"];
    if (!url) throw "Redis URL must be set";
    identifier = url;
  } else if (proto === "quic") {
    const addr = obj["quic-addr"];
    if (!addr) throw "QUIC address must be set";
    identifier = "wrpc+quic://" + addr;

    const bucket = obj["quic-bucket"];
    if (bucket) identifier = identifier + ";" + bucket;
  } else if (proto === "tcp") {
    const addr = obj["tcp-addr"];
    if (!addr) throw "TCP address must be set";
    identifier = "wrpc+tcp://" + addr;

    const bucket = obj["tcp-bucket"];
    if (bucket) identifier = identifier + ";" + bucket;
  } else if (proto === "unix") {
    const path = obj["unix-path"];
    if (!path) throw "Unix Domain Socket path must be set";
    identifier = "wrpc+unix://" + path;

    const bucket = obj["unix-bucket"];
    if (bucket) identifier = identifier + ";" + bucket;
  } else if (proto === "web") {
    const addr = obj["web-addr"];
    if (!addr) throw "WebTransport address must be set";
    identifier = "wrpc+web://" + addr;

    const bucket = obj["web-bucket"];
    if (bucket) identifier = identifier + ";" + bucket;
  } else {
    throw "selected wRPC transport not supported yet";
  }

  const existing = buckets.get(identifier);
  if (existing) return existing;

  dbg.debug(`opening bucket for \`${identifier}\`...`);
  const [res] = await invoke(STORE, "open", OPEN.params, OPEN.results, [identifier]);
  if (res.kind === "error") throw "failed to open bucket: " + formatError(res.value);
  const bucket = res.value;
  buckets.set(identifier, bucket);
  dbg.log(`opened bucket ${uuidString(bucket)}`);
  return bucket;
}

async function handleGet() {
  /** @type {HTMLInputElement | null} */
  const getValue = document.querySelector('#get input[name="get-value"]');
  if (getValue) getValue.value = null;

  const bucket = await getBucket();

  const obj = getFormValues("#get");
  const key = obj["get-key"];
  if (!key) throw "key must be set";

  dbg.debug(`getting \`${key}\`...`);
  const [res] = await invoke(STORE, "bucket.get", GET.params, GET.results, [bucket, key]);
  if (res.kind === "error") throw "failed to get value: " + formatError(res.value);
  if (res.value === undefined) {
    dbg.info("key missing");
    return;
  }
  const s = new TextDecoder().decode(res.value);
  dbg.info(`got value from bucket ${uuidString(bucket)}:`, JSON.stringify(s, null, 2));
  if (getValue) getValue.value = s;
}

async function handleSet() {
  const bucket = await getBucket();

  const obj = getFormValues("#set");
  const key = obj["set-key"];
  if (!key) throw "key must be set";

  const value = obj["set-value"];
  const valueBuf = value ? new TextEncoder().encode(value) : new Uint8Array();

  dbg.debug(`setting \`${key}\`...`);
  const [res] = await invoke(STORE, "bucket.set", SET.params, SET.results, [bucket, key, valueBuf]);
  if (res.kind === "error") throw "failed to set value: " + formatError(res.value);
  dbg.info(`set value in bucket ${uuidString(bucket)}`);
}

const dbg = (() => {
  const output = document.querySelector("#message-output");
  const className = {
    debug: "has-text-grey",
    log: "has-text-normal",
    info: "has-text-info",
    warn: "has-text-warning",
    error: "has-text-danger",
  };
  const handleLog = (value, level = "log") => {
    level = ["debug", "log", "info", "warn", "error"].includes(level) ? level : "info";
    const prefix = new Date().toISOString();
    const levelTag = `[${level}]`.padStart(7, " ");
    const span = document.createElement("span");
    span.textContent = `${prefix} ${levelTag} ${value.join(" ")}\n`;
    span.classList.add(className[level]);
    span.dataset.level = level;
    if (!output) {
      console.error(span.textContent);
      return;
    }
    output.prepend(span);
  };
  return {
    debug: (...value) => handleLog(value, "debug"),
    log: (...value) => handleLog(value, "log"),
    info: (...value) => handleLog(value, "info"),
    warn: (...value) => handleLog(value, "warn"),
    error: (...value) => handleLog(value, "error"),
  };
})();

// @ts-check
function initUI() {
  function updateTemplate() {
    const option = protoDropdown?.value ?? "mem";
    const defaultTemplate = document.querySelector(".form-fields[data-option=default]");
    const templateOutput = document.querySelector("#template-output");
    const template =
      document.querySelector(`.form-fields[data-option=${option}]`) ?? defaultTemplate;
    if (templateOutput && template) templateOutput.innerHTML = template.innerHTML;
  }

  /** @type {HTMLSelectElement | null} */
  const protoDropdown = document.querySelector("#proto");
  protoDropdown?.addEventListener("change", updateTemplate);
  updateTemplate();

  /** @type {HTMLFormElement | null} */
  const settingsForm = document.querySelector("#settings");
  settingsForm?.addEventListener("submit", (e) => e.preventDefault());

  /** @type {HTMLFormElement | null} */
  const getForm = document.querySelector("#get");
  getForm?.addEventListener("submit", (e) => {
    e.preventDefault();
    handleGet().catch(dbg.error);
  });

  /** @type {HTMLFormElement | null} */
  const setForm = document.querySelector("#set");
  setForm?.addEventListener("submit", (e) => {
    e.preventDefault();
    handleSet().catch(dbg.error);
  });
}

function getFormValues(selector) {
  /** @type {HTMLFormElement | null} */
  const form = document.querySelector(selector);
  if (!form) throw new Error("form not found");
  const formData = new FormData(form);
  const formEntries = Array.from(formData);
  return Object.fromEntries(formEntries.map(([key, value]) => [key, value.toString()]));
}

initUI();

const { PORT, CERT_DIGEST } = await import("./consts.js");
for (;;) {
  dbg.log("connecting to wRPC over WebTransport on `" + PORT + "`...");
  let c;
  try {
    c = new WebTransport("https://localhost:" + PORT, {
      serverCertificateHashes: [
        {
          algorithm: "sha-256",
          value: CERT_DIGEST.buffer,
        },
      ],
    });
  } catch (err) {
    dbg.error("failed to connect to WebTransport endpoint: " + err);
    await new Promise((r) => setTimeout(r, 1000));
    continue;
  }

  dbg.debug("waiting for WebTransport connection to be established...");
  try {
    await c.ready;
  } catch (err) {
    dbg.error("failed to establish WebTransport connection: " + err);
    await new Promise((r) => setTimeout(r, 1000));
    continue;
  }

  conn = c;
  dbg.info("WebTransport connection established");
  try {
    const { closeCode, reason } = await conn.closed;
    dbg.log(`WebTransport connection closed with code '${closeCode}': ${reason}`);
  } catch (err) {
    dbg.error("WebTransport connection failed: " + err);
  }
  conn = null;
}
