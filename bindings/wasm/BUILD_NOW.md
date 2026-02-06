# How to build WASM

## Quick option (single command)

**Build + patch + server** (open the test in the browser immediately):

```bash
cd bindings/wasm
./build-and-serve.sh
```

Default port is 8000. For another port: `./build-and-serve.sh 8080`. Stop the server: Ctrl+C.

---

**Build and patch only** (server not started):

```bash
cd bindings/wasm
./test-now.sh
```

The script will:
- check `wasm-pack` and target `wasm32-unknown-unknown`;
- source `setup-env.sh` if needed (WASI SDK);
- run `cargo check --target wasm32-unknown-unknown`;
- run `wasm-pack build --target web --out-dir pkg`;
- run `patch-pkg-env.sh` (env + libc polyfills for the browser).

Result: `pkg/` folder with the ready module.

---

## Manual build step by step

```bash
cd bindings/wasm

# 1. (Optional) WASI SDK for building C code (secp256k1)
source ./setup-env.sh

# 2. Compilation check
cargo check --target wasm32-unknown-unknown

# 3. WASM build (without host CC/AR so wasm-bindgen-cli is not broken)
unset CC AR TARGET_CC TARGET_AR CFLAGS 2>/dev/null || true
wasm-pack build --target web --out-dir pkg

# 4. Patch pkg for browser (env + libc polyfills). If you used ./test-now.sh — patch is already applied.
./patch-pkg-env.sh
```

---

## Where "Import #N \"env\" \"function\"" errors come from and how to fix them

They come from **C dependencies** (secp256k1, SQLite, etc.): when building for wasm32 they pull in libc/POSIX symbols (`strcmp`, `getenv`, `malloc`, `access`, `getcwd`, `pthread_*`, etc.). The browser has no libc, so these functions must be provided in the `imports['env']` object when loading the WASM.

**How to fix:** the patch already adds a **set of libc/POSIX polyfills** in one block. After `wasm-pack build` always run `./patch-pkg-env.sh` (or use `./test-now.sh` / `./build-and-serve.sh` — they run the patch automatically). If a new `env` symbol appears, add one function to the `__wbg_star0` object in `pkg/rgb_lib_wasm.js` and to the `INLINE` block in `patch-pkg-env.sh`.

---

## Why do imports keep appearing and how long are polyfills needed?

- **Why they keep appearing:** C code (SQLite, secp256k1, etc.) when linked for wasm32 leaves **unresolved symbols** — every libc/POSIX call (strcmp, getenv, malloc, access, getcwd, stat, pthread_*, etc.) becomes an `env.symbol` import. The linker does not supply the implementation; it only records the import list. As many such calls exist in the code, that many imports there will be.

- **How long polyfills are needed:** As long as the dependency graph includes **C** (SQLite, secp256k1-sys, etc.), loading the WASM in the browser requires providing all these symbols in `imports['env']`. So polyfills are needed **the whole time** this build is used. They don't "run out" — we just add stubs one by one for each new import that appears on first run.

- **How to reduce iterations:** The patch already adds a large set of common symbols (strings, memory, files, time, pthread, getcwd, stat, realpath, etc.). If a new import still appears after that — add one function to `__wbg_star0` and to `patch-pkg-env.sh`; after a dozen or two such additions the set usually stabilizes for that SQLite/dependency version.

- **How to get rid of polyfills entirely:** Remove C dependencies: e.g. switch to pure Rust implementations (different SQLite-compatible layer or different DB, different crypto stack without C). Then there will be no `env` imports and no patch needed (as in bdk-wasm).

---

## Why doesn't WASM "just get generated" with the right env imports?

The WASM file is produced by the **linker** from Rust code and **C code** (secp256k1, SQLite, etc.). The C code is compiled for wasm32 and leaves **unresolved symbols** in object files (strcmp, getenv, strcspn, etc.) — they are expected from the "external" environment. So the `.wasm` ends up with **imports** like `env.strcmp`, `env.strcspn`, not their implementation. The implementation must be provided by the **host** when calling `WebAssembly.instantiate(module, imports)`. The browser has no libc, so we supply polyfills in `imports['env']`.

**wasm-pack** only generates the JS glue from the Rust/wasm-bindgen part; it does not see which exact `env` symbols the final WASM will need (that is decided by the linker when building C dependencies). So you cannot "generate WASM at once with the right changes" on the wasm-pack side — the "changes" are not in the WASM but in the **JS**, in the `imports.env` object, and our patch is what extends it.

**Summary:**
1. **Patch after build** — the only place where we add polyfills to `imports['env']`.
2. **Build already runs the patch:** `test-now.sh` and `build-and-serve.sh` call `patch-pkg-env.sh` after `wasm-pack build`.
3. New `env` import — add one function to `__wbg_star0` in `pkg/rgb_lib_wasm.js` and to `INLINE` in `patch-pkg-env.sh`.

---

## Running the test in the browser

```bash
cd bindings/wasm
python3 -m http.server 8000
```

Open: **http://localhost:8000/examples/simple-test.html**

The server must run from the `bindings/wasm` directory (common parent for `pkg/` and `examples/`).

---

## What to do next

1. **Build** (if you haven't yet or after changes):
   ```bash
   cd bindings/wasm
   ./test-now.sh
   ```

2. **Start the server and test in the browser**:
   ```bash
   cd bindings/wasm
   python3 -m http.server 8000
   ```
   Open: http://localhost:8000/examples/simple-test.html → click "Basic test" or "Full online test".

3. **If you get an error like**  
   `Import #N "env" "function_name": function import requires a callable`  
   — note the function name (e.g. `fstat`, `write`), and add it to the polyfills in `patch-pkg-env.sh` and in `pkg/rgb_lib_wasm.js`.

4. **When the test passes** — you can call WASM exports from your page: `generate_keys`, `restore_keys`, `create_wallet_data`, etc. (see `pkg/rgb_lib_wasm.js` or `.d.ts`).
