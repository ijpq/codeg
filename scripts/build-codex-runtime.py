"""Reproduce the pinned private CLI; never touches an installed Codex or CODEX_HOME."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import os
import sys
import tempfile
import zipfile

ROOT = Path(__file__).resolve().parents[1]
VENDOR = ROOT / 'vendor/codex-runtime'
MIGRATION_DIRECTORIES = (
    'migrations', 'logs_migrations', 'goals_migrations', 'memory_migrations',
    'queue_migrations', 'thread_history_migrations',
)

def prepare_windows_checkout_resources(source, protected_paths):
    # Reproduce Git's official Windows checkout conversion for unchanged
    # resources, honoring attributes/binary detection. Keep patched sources
    # byte-exact so patch validation remains independent of platform EOLs.
    git = ['git', '-c', 'core.autocrlf=true', '-c', 'core.safecrlf=false',
           '-c', 'core.longpaths=true']
    modified = subprocess.check_output(git + ['diff', '--name-only', '-z'], cwd=source)
    unexpected = {p.decode() for p in modified.split(b'\0') if p} - set(protected_paths)
    if unexpected:
        raise RuntimeError(f'Unexpected resource source changes: {sorted(unexpected)}')
    entries = subprocess.check_output(git + ['ls-files', '--eol', '-z'], cwd=source)
    selected = []
    for entry in entries.split(b'\0'):
        if not entry:
            continue
        info, name = entry.decode().split('\t', 1)
        if name not in protected_paths and 'eol=lf' not in info and 'attr/-text' not in info and 'i/lf' in info and 'w/lf' in info:
            selected.append(name)
    if selected:
        # Checkout to a fresh prefix: checkout-index may retain an unchanged
        # working file's LF bytes when its existing index stat cache is valid.
        # A short sibling prefix also avoids Windows' legacy path-length limit.
        with tempfile.TemporaryDirectory(prefix='.wr-', dir=source.parent) as scratch:
            prefix = Path(scratch).as_posix() + '/'
            subprocess.run(git + ['checkout-index', '--prefix=' + prefix, '--stdin', '-z'], cwd=source,
                           input=('\0'.join(selected) + '\0').encode(), check=True)
            for name in selected:
                expected = (Path(scratch) / name).read_bytes()
                target = source / name
                if target.read_bytes() != expected:
                    target.write_bytes(expected)
    return selected

def prepare_migration_sources(source, windows):
    # SQLx hashes the embedded SQL bytes. The official Windows CLI embeds
    # CRLF migrations; matching its checkout format preserves existing DBs.
    # Normalize only build sources, never databases or their stored checksums.
    checksums = {}
    for directory in MIGRATION_DIRECTORIES:
        files = sorted((source / 'codex-rs' / 'state' / directory).glob('*.sql'))
        if not files:
            raise RuntimeError(f'Missing pinned migration sources: {directory}')
        for path in files:
            data = path.read_bytes()
            canonical = data.replace(b'\r\n', b'\n')
            if b'\r' in canonical:
                raise RuntimeError(f'Unexpected migration line endings: {path}')
            expected = canonical.replace(b'\n', b'\r\n') if windows else data
            if data != expected:
                path.write_bytes(expected)
            checksums[path.relative_to(source).as_posix()] = hashlib.sha384(expected).hexdigest()
    return checksums

def run(command, cwd, env=None):
    subprocess.run(command, cwd=cwd, env=env, check=True)

def repair_windows_v8_cache(cargo, workspace, target):
    # rusty_v8 stores its native library outside Cargo's build-script OUT_DIR.
    # A restored cache can retain the script output while omitting gn_out, so
    # Cargo considers the script fresh and the next link fails. Invalidate only
    # this package; its original build script restores the checksum-pinned lib.
    profile = target / 'release'
    library = profile / 'gn_out' / 'obj' / 'rusty_v8.lib'
    if any((profile / 'build').glob('v8-*/output')) and (
        not library.is_file() or library.stat().st_size == 0
    ):
        print('Repairing restored V8 cache: native Windows library is missing.', flush=True)
        run(cargo + ['clean', '--release', '--package', 'v8'], workspace)
        return True
    return False

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
    if os.name == 'nt':
        protected = {line.split(' b/', 1)[1] for line in patch.read_text().splitlines()
                     if line.startswith('diff --git ')}
        prepare_windows_checkout_resources(source, protected)
    migration_checksums = prepare_migration_sources(source, windows=os.name == 'nt')
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
        repair_windows_v8_cache(cargo, workspace, target)
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
    (output / 'migration-sources.json').write_text(json.dumps({
        'line_endings': 'CRLF' if os.name == 'nt' else 'LF',
        'sha384': migration_checksums}, indent=2) + '\n')
    (output / 'codex.sha256').write_text(hashlib.sha256(binary.read_bytes()).hexdigest() + '  ' + binary.name + '\n')
    (output / 'runtime.sha256').write_text(''.join(hashlib.sha256((output / name).read_bytes()).hexdigest() + '  ' + name + '\n' for name in files))

if __name__ == '__main__':
    main()
