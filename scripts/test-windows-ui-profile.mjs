import assert from "node:assert/strict"
import { execFileSync } from "node:child_process"
import { mkdtemp, rm, stat } from "node:fs/promises"
import { tmpdir } from "node:os"
import { join, relative, isAbsolute } from "node:path"
import { test } from "node:test"
import {
  isolatedCodegEnv,
  prepareWindowsProfile,
} from "./windows-ui-profile.mjs"

test("Server overrides cannot escape into the isolated Setup child", () => {
  const parent = {
    PATH: "kept",
    PI_CODING_AGENT_DIR: "native-pi-profile",
    CODEG_DATA_DIR: "production-data",
    codeg_home: "production-home",
    CODEG_PORT: "production-port",
    CODEG_STATIC_DIR: "production-web",
    CODEG_PI_ACP_PATH: "production-adapter",
    CODEG_TOKEN: "synthetic-secret-do-not-inherit",
  }
  const child = isolatedCodegEnv(parent, {
    CODEG_DATA_DIR: "fixture-data",
    CODEG_HOME: "fixture-home",
  })
  assert.deepEqual(child, {
    PATH: "kept",
    PI_CODING_AGENT_DIR: "native-pi-profile",
    CODEG_DATA_DIR: "fixture-data",
    CODEG_HOME: "fixture-home",
  })
  assert.equal(parent.CODEG_DATA_DIR, "production-data")
  assert.ok(parent.CODEG_TOKEN)
})

function nativeDirectories(env) {
  return JSON.parse(
    execFileSync(
      "python",
      [
        "-c",
        `import ctypes, json, uuid
shell = ctypes.WinDLL('shell32')
ole = ctypes.WinDLL('ole32')
shell.SHGetKnownFolderPath.argtypes = [ctypes.c_void_p, ctypes.c_uint32, ctypes.c_void_p, ctypes.POINTER(ctypes.c_wchar_p)]
shell.SHGetKnownFolderPath.restype = ctypes.c_long
ole.CoTaskMemFree.argtypes = [ctypes.c_void_p]
result = {}
for name, identity in [('data', '3EB685DB-65F9-4CF6-A03A-E3EF65729F3D'), ('local', 'F1B32785-6FBA-4FCF-9D55-7B8E7F157091')]:
    folder = ctypes.create_string_buffer(uuid.UUID(identity).bytes_le)
    path = ctypes.c_wchar_p()
    status = shell.SHGetKnownFolderPath(folder, 0, None, ctypes.byref(path))
    result[name] = {'status': status, 'path': path.value}
    ole.CoTaskMemFree(path)
print(json.dumps(result))`,
      ],
      { env, encoding: "utf8" }
    )
  )
}

test("isolated profile prepares native app directories without changing the parent", async () => {
  const root = await mkdtemp(join(tmpdir(), "codeg-profile-check-"))
  const original = { ...process.env }
  const env = { ...original, USERPROFILE: root }
  try {
    if (process.platform === "win32") {
      // Keep the before-result as evidence; Windows may have a different
      // registered Known Folder configuration, so do not require a failure.
      console.log("empty-profile native paths", nativeDirectories(env))
    }
    await prepareWindowsProfile(env, root)
    assert.equal(env.USERPROFILE, root)
    for (const name of ["APPDATA", "LOCALAPPDATA"]) {
      assert.ok((await stat(env[name])).isDirectory())
      const rel = relative(root, env[name])
      assert.ok(rel && !rel.startsWith("..") && !isAbsolute(rel))
    }
    if (process.platform === "win32") {
      const actual = nativeDirectories(env)
      console.log("prepared-profile native paths", actual)
      assert.equal(actual.data.status, 0)
      assert.equal(actual.local.status, 0)
      assert.equal(actual.data.path.toLowerCase(), env.APPDATA.toLowerCase())
      assert.equal(
        actual.local.path.toLowerCase(),
        env.LOCALAPPDATA.toLowerCase()
      )
    }
    assert.equal(Object.keys(process.env).length, Object.keys(original).length)
    assert.ok(
      Object.entries(original).every(
        ([key, value]) => process.env[key] === value
      ),
      "The parent environment must be unchanged"
    )
  } finally {
    await rm(root, { recursive: true })
  }
})
