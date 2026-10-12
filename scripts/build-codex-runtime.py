"""Reproduce the pinned private CLI; never touches an installed Codex or CODEX_HOME."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import os
import sys
import zipfile

ROOT = Path(__file__).resolve().parents[1]
VENDOR = ROOT / 'vendor/codex-runtime'

def run(command, cwd, env=None):
    subprocess.run(command, cwd=cwd, env=env, check=True)

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source-dir', type=Path, required=True)
    parser.add_argument('--output-dir', type=Path, required=True)
    parser.add_argument('--prepare-only', action='store_true')
    args = parser.parse_args()
    manifest = json.loads((VENDOR / 'source.json').read_text())
    patch = VENDOR / manifest['patch']
    assert hashlib.sha256(patch.read_bytes()).hexdigest() == manifest['patch_sha256']
    source = args.source_dir.resolve()
    if not source.exists():
        run(['git', 'clone', '--config', 'core.autocrlf=false', '--depth', '1', '--branch', manifest['tag'], manifest['repository'], str(source)], ROOT)
    sha = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=source, text=True).strip()
    if sha != manifest['commit']:
        raise RuntimeError(f'Unexpected CLI source commit: {sha}')
    if subprocess.run(['git', 'apply', '--reverse', '--check', str(patch)], cwd=source, capture_output=True).returncode:
        run(['git', 'apply', '--check', str(patch)], source)
        run(['git', 'apply', str(patch)], source)
    if args.prepare_only:
        return
    cargo = ['cargo', '+' + manifest['rust_toolchain']]
    workspace = source / 'codex-rs'
    if os.name == 'nt':
        # Keep upstream's MSVC stack/static-CRT settings when adding path remaps.
        # Encode flags instead of shell splitting paths containing spaces.
        flags = ['-C', 'link-arg=/STACK:8388608', '-C', 'target-feature=+crt-static',
                 '--remap-path-prefix=' + str(source) + '=/codeg-codex-source',
                 '--remap-path-prefix=' + str(Path.home()) + '=/codeg-build-home']
        os.environ['CARGO_ENCODED_RUSTFLAGS'] = '\x1f'.join(flags)
    os.environ['CARGO_PROFILE_RELEASE_STRIP'] = 'symbols'
    target = Path(os.environ.get('CARGO_TARGET_DIR', workspace / 'target')).resolve()
    if os.name == 'nt':
        # Upstream builds the sandbox-enabled V8 pair itself; the crate's
        # default denoland URL does not publish the required Windows variant.
        # Reuse the exact checkout's checksum-pinned artifact resolver.
        os.environ['CODEX_REPO_ROOT'] = str(source)
        sys.path.insert(0, str(source / 'scripts'))
        from codex_package.targets import TARGET_SPECS
        from codex_package.v8 import resolve_codex_v8_cargo_env
        os.environ.update(resolve_codex_v8_cargo_env(TARGET_SPECS['x86_64-pc-windows-msvc'], environ={}))
        run(cargo + ['build', '--locked', '--release', '--package', 'codex-code-mode-host', '--package', 'codex-windows-sandbox', '--package', 'codex-windows-sandbox-service', '--bin', 'codex-code-mode-host', '--bin', 'codex-command-runner', '--bin', 'codex-windows-sandbox-setup', '--bin', 'codex-windows-sandbox-service'], workspace)
    test_env = dict(os.environ, RUST_MIN_STACK='8388608', CODEG_LEGACY_CHECKPOINT_RESUME='1')
    if os.name == 'nt':
        for name in ['codex-command-runner', 'codex-windows-sandbox-setup']:
            test_env['CARGO_BIN_EXE_' + name.replace('-', '_')] = str(target / 'release' / (name + '.exe'))
    run(cargo + ['test', '--locked', '--package', 'codex-thread-store', '--lib'], workspace)
    run(cargo + ['test', '--locked', '--package', 'codex-core', '--lib', 'codeg_checkpoint_'], workspace)
    run(cargo + ['clippy', '--locked', '--package', 'codex-thread-store', '--lib', '--', '-D', 'warnings'], workspace)
    run(cargo + ['clippy', '--locked', '--package', 'codex-core', '--lib', '--', '-D', 'warnings'], workspace)
    run(cargo + ['test', '--locked', '--package', 'codex-app-server', '--test', 'all', 'thread_resume', '--', '--test-threads=2'], workspace, test_env)
    run(cargo + ['build', '--locked', '--release', '--package', 'codex-cli', '--bin', 'codex'], workspace)
    suffix = '.exe' if os.name == 'nt' else ''
    binary = target / 'release' / ('codex' + suffix)
    version = subprocess.check_output([str(binary), '--version'], text=True).strip()
    if version != 'codex-cli ' + manifest['version']:
        raise RuntimeError(f'Unexpected CLI binary version: {version}')
    output = args.output_dir.resolve()
    output.mkdir(parents=True, exist_ok=True)
    shutil.copy2(binary, output / binary.name)
    files = [binary.name]
    if os.name == 'nt':
        # Match the source's supported Windows root package layout so helpers
        # and ripgrep resolve independently of the machine's global CLI/PATH.
        resources = output / 'codex-resources'
        path_dir = output / 'codex-path'
        resources.mkdir(exist_ok=True)
        path_dir.mkdir(exist_ok=True)
        shutil.copy2(target / 'release/codex-code-mode-host.exe', output / 'codex-code-mode-host.exe')
        files.append('codex-code-mode-host.exe')
        for name in ['codex-command-runner.exe', 'codex-windows-sandbox-setup.exe', 'codex-windows-sandbox-service.exe']:
            shutil.copy2(target / 'release' / name, resources / name)
            files.append('codex-resources/' + name)
        os.environ['CODEX_REPO_ROOT'] = str(source)
        sys.path.insert(0, str(source / 'scripts'))
        from codex_package.ripgrep import RG_MANIFEST, resolve_rg_bin
        from codex_package.targets import TARGET_SPECS
        from codex_package.dotslash import artifact_for_target, archive_filename, default_cache_root
        spec = TARGET_SPECS['x86_64-pc-windows-msvc']
        rg = resolve_rg_bin(spec, None)
        shutil.copy2(rg, path_dir / 'rg.exe')
        files.append('codex-path/rg.exe')
        artifact = artifact_for_target(spec, RG_MANIFEST, artifact_label='ripgrep')
        archive = default_cache_root() / (spec.target + '-rg') / archive_filename(artifact.url)
        with zipfile.ZipFile(archive) as package:
            for name in package.namelist():
                if Path(name).name in ['LICENSE-MIT', 'UNLICENSE']:
                    with package.open(name) as license_source, (resources / ('ripgrep-' + Path(name).name)).open('wb') as license_output:
                        shutil.copyfileobj(license_source, license_output)
        (output / 'codex-package.json').write_text(json.dumps({
            'layoutVersion': 1, 'version': manifest['version'], 'target': spec.target,
            'variant': 'codex', 'entrypoint': binary.name,
            'resourcesDir': 'codex-resources', 'pathDir': 'codex-path'}, indent=2) + '\n')
    for name in ['source.json', 'LICENSE', 'NOTICE']:
        if (VENDOR / name).exists():
            shutil.copy2(VENDOR / name, output / name)
    (output / 'codex.sha256').write_text(hashlib.sha256(binary.read_bytes()).hexdigest() + '  ' + binary.name + '\n')
    (output / 'runtime.sha256').write_text(''.join(hashlib.sha256((output / name).read_bytes()).hexdigest() + '  ' + name + '\n' for name in files))

if __name__ == '__main__':
    main()
