#!/bin/bash
# Патч pkg/rgb_lib_wasm.js: заменяем импорт 'env' на inline объект для браузера.
# Запускать после wasm-pack build (или вручную, если кнопка "Запустить тест" не работает).

cd "$(dirname "$0")"
PKG_JS="pkg/rgb_lib_wasm.js"

if [ ! -f "$PKG_JS" ]; then
  echo "❌ Не найден: $PKG_JS (сначала выполните wasm-pack build)"
  exit 1
fi

if grep -q "import \* as __wbg_star0 from 'env'" "$PKG_JS"; then
  # Полный блок env: __wbindgen_throw + полифиллы libc (C-зависимости WASM тянут эти символы).
  INLINE='// Патч для браузера: импорт '\''env'\'' не резолвится + полифиллы libc.
let __env_memory = null;
function __env_get_mem() {
  return __env_memory ? new Uint8Array(__env_memory.buffer) : (wasm && wasm.memory ? new Uint8Array(wasm.memory.buffer) : null);
}
function __env_read_cstr(ptr) {
  const mem = __env_get_mem();
  if (!mem) return "";
  let end = ptr >>> 0;
  while (mem[end]) end++;
  return new TextDecoder().decode(mem.subarray(ptr >>> 0, end));
}
const __wbg_star0 = {
  __wbindgen_throw(ptr, len) {
    throw new Error(getStringFromWasm0(ptr, len));
  },
  strcmp(ptr1, ptr2) {
    const s1 = __env_read_cstr(ptr1);
    const s2 = __env_read_cstr(ptr2);
    if (s1 < s2) return -1;
    if (s1 > s2) return 1;
    return 0;
  },
  strncmp(ptr1, ptr2, n) {
    const mem = __env_get_mem();
    if (!mem || !n) return 0;
    const a = ptr1 >>> 0, b = ptr2 >>> 0, len = n >>> 0;
    for (let i = 0; i < len; i++) {
      const x = mem[a + i], y = mem[b + i];
      if (x !== y) return (x >>> 0) - (y >>> 0);
      if (x === 0) return 0;
    }
    return 0;
  },
  memcmp(ptr1, ptr2, n) {
    const mem = __env_get_mem();
    if (!mem || !n) return 0;
    const a = ptr1 >>> 0, b = ptr2 >>> 0, len = n >>> 0;
    for (let i = 0; i < len; i++) {
      const d = mem[a + i] - mem[b + i];
      if (d) return d;
    }
    return 0;
  },
  memset(ptr, c, n) {
    const mem = __env_get_mem();
    if (mem && n) mem.fill(c & 0xff, ptr >>> 0, (ptr >>> 0) + (n >>> 0));
    return ptr;
  },
  memcpy(dest, src, n) {
    const mem = __env_get_mem();
    if (mem && n) mem.copyWithin(dest >>> 0, src >>> 0, (src >>> 0) + (n >>> 0));
    return dest;
  },
  strlen(ptr) {
    const mem = __env_get_mem();
    if (!mem) return 0;
    let i = ptr >>> 0;
    while (mem[i]) i++;
    return i - (ptr >>> 0);
  },
  strcspn(s_ptr, reject_ptr) {
    const mem = __env_get_mem();
    if (!mem) return 0;
    const s = s_ptr >>> 0, rej = reject_ptr >>> 0;
    const rejectSet = new Set();
    for (let j = rej; mem[j]; j++) rejectSet.add(mem[j]);
    let i = s;
    while (mem[i] && !rejectSet.has(mem[i])) i++;
    return i - s;
  },
  strspn(s_ptr, accept_ptr) {
    const mem = __env_get_mem();
    if (!mem) return 0;
    const s = s_ptr >>> 0, acc = accept_ptr >>> 0;
    const acceptSet = new Set();
    for (let j = acc; mem[j]; j++) acceptSet.add(mem[j]);
    let i = s;
    while (mem[i] && acceptSet.has(mem[i])) i++;
    return i - s;
  },
  strchr(s_ptr, c) {
    const mem = __env_get_mem();
    if (!mem) return 0;
    const s = s_ptr >>> 0, b = (c & 0xff) >>> 0;
    let i = s;
    while (mem[i]) {
      if ((mem[i] >>> 0) === b) return i;
      i++;
    }
    return (b === 0) ? i : 0;
  },
  strrchr(s_ptr, c) {
    const mem = __env_get_mem();
    if (!mem) return 0;
    const s = s_ptr >>> 0, b = (c & 0xff) >>> 0;
    let last = 0, i = s;
    while (mem[i]) {
      if ((mem[i] >>> 0) === b) last = i;
      i++;
    }
    return (b === 0) ? i : last;
  },
  memchr(s_ptr, c, n) {
    const mem = __env_get_mem();
    if (!mem || !n) return 0;
    const s = s_ptr >>> 0, b = (c & 0xff) >>> 0, len = n >>> 0;
    for (let i = 0; i < len; i++) {
      if ((mem[s + i] >>> 0) === b) return s + i;
    }
    return 0;
  },
  memrchr(s_ptr, c, n) {
    const mem = __env_get_mem();
    if (!mem || !n) return 0;
    const s = s_ptr >>> 0, b = (c & 0xff) >>> 0, len = n >>> 0;
    for (let i = len - 1; i >= 0; i--) {
      if ((mem[s + i] >>> 0) === b) return s + i;
    }
    return 0;
  },
  malloc(size) {
    if (wasm && wasm.__wbindgen_malloc) return wasm.__wbindgen_malloc(size >>> 0);
    return 0;
  },
  free(ptr) {
    if (wasm && wasm.__wbindgen_free) wasm.__wbindgen_free(ptr >>> 0, 1);
  },
  realloc(ptr, size) {
    if (wasm && wasm.__wbindgen_realloc) return wasm.__wbindgen_realloc(ptr >>> 0, size >>> 0, 1);
    return 0;
  },
  calloc(nmemb, size) {
    const n = (nmemb >>> 0) * (size >>> 0);
    const p = (wasm && wasm.__wbindgen_malloc) ? wasm.__wbindgen_malloc(n) : 0;
    if (p && wasm && wasm.memory) new Uint8Array(wasm.memory.buffer).fill(0, p, p + n);
    return p;
  },
  getenv(_name_ptr) { return 0; },
  sqlite3_load_extension(_db, _zFile, _zProc, _pzErrMsg) { return 1; },
  qsort(base, nmemb, size, compar) {
    const n = nmemb >>> 0, sz = size >>> 0;
    if (!wasm || !wasm.memory || n <= 1 || !sz) return;
    const table = wasm.table || wasm.__indirect_function_table;
    if (!table || typeof table.get !== \"function\") return;
    const cmp = table.get(compar >>> 0);
    if (typeof cmp !== \"function\") return;
    const baseIdx = base >>> 0;
    const mem = new Uint8Array(wasm.memory.buffer);
    const swap = (i, j) => {
      const a = baseIdx + i * sz, b = baseIdx + j * sz;
      for (let k = 0; k < sz; k++) { const t = mem[a + k]; mem[a + k] = mem[b + k]; mem[b + k] = t; }
    };
    const partition = (lo, hi) => {
      let i = lo, j = hi + 1;
      const pivot = lo;
      while (true) {
        while (i < hi && cmp(baseIdx + (++i) * sz, baseIdx + pivot * sz) < 0);
        while (j > lo && cmp(baseIdx + (--j) * sz, baseIdx + pivot * sz) > 0);
        if (i >= j) break;
        swap(i, j);
      }
      swap(pivot, j);
      return j;
    };
    const sort = (lo, hi) => {
      if (lo < hi) { const p = partition(lo, hi); sort(lo, p - 1); sort(p + 1, hi); }
    };
    sort(0, n - 1);
  },
  fsync(_fd) { return 0; },
  fflush(_stream) { return 0; },
  time(_t_ptr) { return Math.floor(Date.now() / 1000); },
  nanosleep(_req, _rem) { return 0; },
  gettimeofday(tv_ptr, _tz_ptr) {
    if (tv_ptr) {
      const mem = __env_memory || (wasm && wasm.memory);
      if (mem) {
        const dv = new DataView(mem.buffer);
        const now = Date.now();
        const sec = Math.floor(now / 1000) >>> 0;
        const usec = ((now % 1000) * 1000) >>> 0;
        dv.setUint32(tv_ptr, sec, true);
        dv.setUint32(tv_ptr + 4, usec, true);
      }
    }
    return 0;
  },
  localtime_r(timep_ptr, result_ptr) {
    const mem = __env_memory || (wasm && wasm.memory);
    if (!mem || !result_ptr) return 0;
    const dv = new DataView(mem.buffer);
    const sec = dv.getInt32(timep_ptr, true) >>> 0;
    const d = new Date(sec * 1000);
    const base = result_ptr >>> 0;
    dv.setInt32(base + 0, d.getSeconds(), true);
    dv.setInt32(base + 4, d.getMinutes(), true);
    dv.setInt32(base + 8, d.getHours(), true);
    dv.setInt32(base + 12, d.getDate(), true);
    dv.setInt32(base + 16, d.getMonth(), true);
    dv.setInt32(base + 20, d.getFullYear() - 1900, true);
    dv.setInt32(base + 24, d.getDay(), true);
    const start = new Date(d.getFullYear(), 0, 0);
    dv.setInt32(base + 28, Math.floor((d - start) / 86400000), true);
    dv.setInt32(base + 32, 0, true);
    return result_ptr;
  },
  pthread_mutexattr_init(_attr) { return 0; },
  pthread_mutexattr_destroy(_attr) { return 0; },
  pthread_mutexattr_settype(_attr, _type) { return 0; },
  pthread_mutex_init(_mutex, _attr) { return 0; },
  pthread_mutex_destroy(_mutex) { return 0; },
  pthread_mutex_lock(_mutex) { return 0; },
  pthread_mutex_unlock(_mutex) { return 0; },
  pthread_mutex_trylock(_mutex) { return 0; },
  pthread_cond_init(_cond, _attr) { return 0; },
  pthread_cond_destroy(_cond) { return 0; },
  pthread_cond_wait(_cond, _mutex) { return 0; },
  pthread_cond_signal(_cond) { return 0; },
  pthread_cond_broadcast(_cond) { return 0; },
  pthread_once(_once_control, _init_fn) { return 0; },
  pthread_key_create(_key, _destructor) { return 0; },
  pthread_key_delete(_key) { return 0; },
  pthread_setspecific(_key, _value) { return 0; },
  pthread_getspecific(_key) { return 0; },
  pthread_create(_thread, _attr, _start, _arg) { return 0; },
  pthread_join(_thread, _retval) { return 0; },
  pthread_detach(_thread) { return 0; },
  pthread_self() { return 0; },
  pthread_equal(_a, _b) { return 1; },
  pthread_exit(_retval) {},
  lseek(_fd, _offset, _whence) { return 0; },
  read(_fd, _buf, count) { return 0; },
  write(_fd, _buf, count) { return count >>> 0; },
  open(_path, _flags, _mode) { return -1; },
  access(_path, _mode) { return -1; },
  getcwd(_buf, _size) { return 0; },
  chdir(_path) { return -1; },
  mkdir(_path, _mode) { return -1; },
  rmdir(_path) { return -1; },
  unlink(_path) { return -1; },
  rename(_old, _new) { return -1; },
  stat(_path, _buf) { return -1; },
  lstat(_path, _buf) { return -1; },
  realpath(_path, _resolved) { return 0; },
  readlink(_path, _buf, _bufsiz) { return -1; },
  symlink(_target, _path) { return -1; },
  link(_old, _new) { return -1; },
  getpid() { return 1; },
  sched_yield() { return 0; },
  sysconf(_name) { return -1; },
  fcntl(_fd, _cmd, _arg) { return 0; },
  dup(_fd) { return -1; },
  dup2(_fd, _fd2) { return -1; },
  close(_fd) { return 0; },
  fstat(_fd, _buf) { return -1; },
  ftruncate(_fd, _length) { return 0; },
  utimes(_path, _times) { return 0; },
  utime(_path, _times) { return 0; },
  futimes(_fd, _times) { return 0; }
};
'
  # Замена первой строки импорта на inline-код (переносимо для macOS/Linux)
  (echo "$INLINE"; tail -n +2 "$PKG_JS") > "$PKG_JS.tmp" && mv "$PKG_JS.tmp" "$PKG_JS"
  # Убираем лишние бэкслэши перед кавычками в qsort (в single-quoted INLINE \" попадает в файл буквально)
  sed 's/!== \\"function\\"/!== "function"/g' "$PKG_JS" > "$PKG_JS.tmp" && mv "$PKG_JS.tmp" "$PKG_JS"
  # Добавляем __env_memory в __wbg_finalize_init (если ещё нет)
  if ! grep -q "__env_memory = instance.exports.memory" "$PKG_JS"; then
    sed 's/\(wasm = instance.exports;\)/\1\
    __env_memory = instance.exports.memory;/' "$PKG_JS" > "$PKG_JS.tmp" && mv "$PKG_JS.tmp" "$PKG_JS"
  fi
  echo "✅ Патч применён к pkg/rgb_lib_wasm.js"
elif ! grep -q "strncmp(ptr1, ptr2, n)" "$PKG_JS"; then
  echo "⚠️  Патч применён, но в __wbg_star0 нет полного набора полифиллов. Пересоберите: wasm-pack build --target web --out-dir pkg && ./patch-pkg-env.sh"
else
  echo "ℹ️  Патч уже применён или импорт 'env' отсутствует"
fi

# Добавить sqlite3_load_extension в env, если его ещё нет (для bundled SQLite в WASM).
if [ -f "$PKG_JS" ] && grep -q "getenv(_name_ptr)" "$PKG_JS" && ! grep -q "sqlite3_load_extension" "$PKG_JS"; then
  sed 's/  getenv(_name_ptr) { return 0; },/  getenv(_name_ptr) { return 0; },\n  sqlite3_load_extension(_db, _zFile, _zProc, _pzErrMsg) { return 1; },/' "$PKG_JS" > "$PKG_JS.tmp" && mv "$PKG_JS.tmp" "$PKG_JS"
  echo "✅ Добавлена заглушка sqlite3_load_extension в env"
fi

# Добавить qsort в env, если его ещё нет (libc-символ для WASM).
if [ -f "$PKG_JS" ] && grep -q "sqlite3_load_extension" "$PKG_JS" && ! grep -q '"env".*qsort\|qsort(base,' "$PKG_JS"; then
  QSORT='  qsort(base, nmemb, size, compar) {
    const n = nmemb >>> 0, sz = size >>> 0;
    if (!wasm || !wasm.memory || n <= 1 || !sz) return;
    const table = wasm.table || wasm.__indirect_function_table;
    if (!table || typeof table.get !== "function") return;
    const cmp = table.get(compar >>> 0);
    if (typeof cmp !== "function") return;
    const baseIdx = base >>> 0;
    const mem = new Uint8Array(wasm.memory.buffer);
    const swap = (i, j) => {
      const a = baseIdx + i * sz, b = baseIdx + j * sz;
      for (let k = 0; k < sz; k++) { const t = mem[a + k]; mem[a + k] = mem[b + k]; mem[b + k] = t; }
    };
    const partition = (lo, hi) => {
      let i = lo, j = hi + 1;
      const pivot = lo;
      while (true) {
        while (i < hi && cmp(baseIdx + (++i) * sz, baseIdx + pivot * sz) < 0);
        while (j > lo && cmp(baseIdx + (--j) * sz, baseIdx + pivot * sz) > 0);
        if (i >= j) break;
        swap(i, j);
      }
      swap(pivot, j);
      return j;
    };
    const sort = (lo, hi) => {
      if (lo < hi) { const p = partition(lo, hi); sort(lo, p - 1); sort(p + 1, hi); }
    };
    sort(0, n - 1);
  },'
  sed "s/  sqlite3_load_extension(_db, _zFile, _zProc, _pzErrMsg) { return 1; },/  sqlite3_load_extension(_db, _zFile, _zProc, _pzErrMsg) { return 1; },\n$QSORT/" "$PKG_JS" > "$PKG_JS.tmp" && mv "$PKG_JS.tmp" "$PKG_JS"
  echo "✅ Добавлен qsort в env"
fi
