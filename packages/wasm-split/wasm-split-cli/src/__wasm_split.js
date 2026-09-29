// when running the harness we need to make sure to uncommon this out...

export function makeLoad(url, deps, fusedImports, initIt) {
  // One load per module: concurrent callers (two modules sharing a chunk, or a preload racing a
  // render) must not instantiate it twice, since its active data segments would re-initialize
  // statics the first instance already uses.
  let loading = null;

  const load = async () => {
    await Promise.all(deps.map((dep) => dep()));
    const response = await fetchWithRetry(url);
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

    let new_exports = await WebAssembly.instantiateStreaming(response, imports);

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

async function fetchWithRetry(url) {
  for (let attempt = 0; ; attempt++) {
    try {
      const response = await fetch(url);
      if (response.ok || attempt >= 2) return response;
    } catch (e) {
      if (attempt >= 2) throw e;
    }
    await new Promise((resolve) => setTimeout(resolve, 500 * (attempt + 1)));
  }
}

let fusedImports = {};
