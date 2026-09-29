// when running the harness we need to make sure to uncommon this out...

export function makeLoad(url, deps, fusedImports, initIt) {
  // One load per module: concurrent callers (two modules sharing a chunk, or a preload racing a
  // render) must not instantiate it twice, since its active data segments would re-initialize
  // statics the first instance already uses.
  let loading = null;

  const load = async () => {
    await Promise.all(deps.map((dep) => dep()));
    const response = await fetchWithRetry(url);
    delete globalThis.__wasm_split_last_failure;
    const initSync = initIt || globalThis.__wasm_split_main_initSync;
    const mainExports = initSync(undefined, undefined);

    let imports = {
      env: {
        memory: mainExports.memory,
      },
      __wasm_split: {
        __indirect_function_table: mainExports.__indirect_function_table,
        __stack_pointer: mainExports.__stack_pointer,
        __tls_base: mainExports.__tls_base,
        memory: mainExports.memory,
      },
    };

    for (let mainExport in mainExports) {
      imports["__wasm_split"][mainExport] = mainExports[mainExport];
    }

    for (let name in fusedImports) {
      imports["__wasm_split"][name] = fusedImports[name];
    }

    // Streaming needs the exact WASM content type, which custom-scheme handlers (e.g. a native
    // shell serving bundled assets) may not send.
    const streamable = (response.headers.get("Content-Type") || "").startsWith("application/wasm");
    let new_exports = streamable
      ? await WebAssembly.instantiateStreaming(response, imports)
      : await WebAssembly.instantiate(await response.arrayBuffer(), imports);

    for (let name in new_exports.instance.exports) {
      fusedImports[name] = new_exports.instance.exports[name];
    }

    return mainExports;
  };

  return async (callbackIndex, callbackData) => {
    if (!loading) {
      loading = load().catch((e) => {
        console.error("Failed to load wasm-split module", e, url, deps, fusedImports);
        // Let a later call retry, e.g. after a network blip.
        loading = null;
        return null;
      });
    }
    const mainExports = await loading;

    if (callbackIndex !== undefined) {
      // Report failure too, so the caller's future settles instead of hanging.
      const exports = mainExports || (initIt || globalThis.__wasm_split_main_initSync)(undefined, undefined);
      exports.__indirect_function_table.get(callbackIndex)(callbackData, mainExports !== null);
    } else if (!mainExports) {
      throw new Error("Failed to load wasm-split module " + url);
    }
  };
}

// Same schedule as a typical asset bootstrap: a 404 is retried because a rolling deploy can serve
// new HTML before every server has the new chunk; other 4xx answers will not change.
const RETRY_DELAYS_MS = [400, 1200, 3000];
// A stalled connection must fail the attempt rather than hold every caller forever.
const ATTEMPT_TIMEOUT_MS = 30000;

async function fetchWithRetry(url) {
  let lastStatus = 0;
  let lastError = null;
  for (let attempt = 0; attempt <= RETRY_DELAYS_MS.length; attempt++) {
    if (attempt > 0) await new Promise((resolve) => setTimeout(resolve, RETRY_DELAYS_MS[attempt - 1]));
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), ATTEMPT_TIMEOUT_MS);
    try {
      const response = await fetch(url, { signal: controller.signal });
      const contentType = response.headers.get("Content-Type") || "";
      if (response.ok && !contentType.includes("text/html")) {
        // Read the body under the same timeout, so a transfer cut off mid-way is retried too.
        const body = await response.arrayBuffer();
        return new Response(body, { status: 200, headers: { "Content-Type": contentType } });
      }
      lastStatus = response.status;
      lastError = new Error("unexpected response " + response.status);
      if (response.status >= 400 && response.status < 500 && response.status !== 404) break;
    } catch (e) {
      lastError = e;
    } finally {
      clearTimeout(timer);
    }
  }
  // Lets the app tell a chunk that is gone after a deploy (reload) from a network failure (retry).
  globalThis.__wasm_split_last_failure = { url, status: lastStatus };
  throw lastError;
}

let fusedImports = {};
