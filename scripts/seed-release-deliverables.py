"""Seed synthetic deliverables only in this invocation's disposable Windows CI data."""
from contextlib import closing
from datetime import datetime, timedelta, timezone
import json
import os
from pathlib import Path
import sqlite3
import sys
import uuid

assert sys.platform == 'win32' and os.environ.get('GITHUB_ACTIONS') == 'true'
root = Path(sys.argv[1]).resolve(strict=True)
assert root.is_relative_to(Path(os.environ['RUNNER_TEMP']).resolve(strict=True))
databases = list((root / 'data').rglob('codeg.db'))
assert len(databases) == 1
workspace = root / 'workspace'
now = datetime.now(timezone.utc)

with closing(sqlite3.connect(databases[0], timeout=30)) as db, db:
    db.execute('PRAGMA foreign_keys=ON')

    def insert(table, values):
        columns = ','.join(values)
        placeholders = ','.join('?' for _ in values)
        db.execute(f'INSERT INTO {table} ({columns}) VALUES ({placeholders})', list(values.values()))

    for conversation_id, count in json.loads(sys.argv[2]):
        assert db.execute('SELECT id FROM conversation WHERE id=?', (conversation_id,)).fetchone()
        for index in range(count):
            stamp = (now - timedelta(seconds=index)).isoformat()
            run_id, deliverable_id = str(uuid.uuid4()), str(uuid.uuid4())
            name = f'conversation-{conversation_id}-output-{index:02}.txt'
            content = f'Isolated deliverable {conversation_id}/{index}\n'
            (workspace / name).write_text(content, encoding='utf-8')
            insert('conversation_turn_run', dict(
                id=run_id, conversation_id=conversation_id, connection_id='release-fixture',
                root_path=str(workspace), status='completed', capture_incomplete=False,
                started_at=stamp, completed_at=stamp, input_paths_json='[]',
                declaration_status='success', expectation_json='{}',
                settlement_status='settled', missing_expected_paths_json='[]',
            ))
            shared = dict(
                conversation_id=conversation_id, turn_run_id=run_id, title=name,
                role='primary', category='standalone_output', change_kind='created',
                position=index, source='declared', created_at=stamp, updated_at=stamp,
            )
            insert('conversation_deliverable', dict(
                **shared, id=deliverable_id, root_path=str(workspace), path=name,
                kind='file', file_name=name, extension='txt', size_bytes=len(content),
                is_valid=True, is_hidden=False, verified_at=stamp,
            ))
            insert('conversation_turn_deliverable', dict(
                **shared, id=str(uuid.uuid4()), deliverable_id=deliverable_id,
            ))
    assert db.execute('PRAGMA foreign_key_check').fetchall() == []
print(json.dumps({'seeded': True, 'conversations': json.loads(sys.argv[2])}))
