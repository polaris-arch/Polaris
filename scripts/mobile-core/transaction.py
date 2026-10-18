"""Fixed component/receipt transactions with byte-checked interruption recovery."""
from pathlib import Path
import os

from fork_input import canonical, decode, require, sha
from artifacts import file_pin, tree_inventory


def pin(path):
    path = Path(path)
    require(not path.is_symlink(), 'transaction object redirected')
    return ({'treeSha256': sha(canonical(tree_inventory(path)))} if path.is_dir() else file_pin(path))


def save(path, value):
    tmp = path.with_name(path.name + '.tmp')
    require(not tmp.exists(), 'interrupted transaction metadata retained')
    with tmp.open('xb') as stream:
        stream.write(canonical(value))
        stream.flush()
        os.fsync(stream.fileno())
    os.replace(tmp, path)


def recover(pairs, work):
    work = Path(work)
    journal = work / 'component-transaction.json'
    if not journal.exists():
        return
    value = decode(journal.read_bytes())
    require(value['schema'] == 'polaris-mobile-pair-transaction-v1' and
            [[row['stage'], row['destination']] for row in value['entries']] ==
            [[str(a), str(b)] for a, b in pairs], 'transaction paths differ from caller-owned fixed slots')
    rejected, previous = work / 'rejected-pair', work / 'previous-pair'
    require(not rejected.is_symlink() and not previous.is_symlink(), 'recovery directory redirected')
    for row in reversed(value['entries']):
        destination = Path(row['destination'])
        backup = previous / row['name']
        failed = rejected / row['name']
        if (destination.exists() and row['originalPin'] is not None and
                pin(destination) == row['originalPin'] and not backup.exists()):
            continue  # Includes an unchanged old object equal to staged bytes.
        if destination.exists() and pin(destination) == row['stagePin']:
            require(not failed.exists(), 'rejected object recovery slot occupied')
            rejected.mkdir(mode=0o700, exist_ok=True)
            destination.rename(failed)
        elif destination.exists():
            require(row['originalPin'] is not None and pin(destination) == row['originalPin'] and not backup.exists(),
                    'unknown destination bytes; interrupted transaction preserved')
        if backup.exists():
            require(row['originalPin'] is not None and pin(backup) == row['originalPin'] and not destination.exists(),
                    'original pair recovery identity differs')
            backup.rename(destination)
        require((not destination.exists()) if row['originalPin'] is None else
                (destination.exists() and pin(destination) == row['originalPin']), 'original pair not restored')
    if previous.exists() and not any(previous.iterdir()):
        previous.rmdir()
    # Retain the interrupted transaction, failed bytes and original journal.
    saved = work / 'recovered-transaction.json'
    require(not saved.exists(), 'recovered transaction record already retained')
    journal.rename(saved)


def install(pairs, work, validate):
    pairs = [(Path(a).absolute(), Path(b).absolute()) for a, b in pairs]
    work = Path(work).absolute()
    require(len({b.name for _, b in pairs}) == len(pairs), 'pair destination names collide')
    require(all(not a.is_symlink() and not b.is_symlink() for a, b in pairs), 'pair endpoint redirected')
    recover(pairs, work)
    previous, journal = work / 'previous-pair', work / 'component-transaction.json'
    require(not previous.exists() and not (work / 'rejected-pair').exists(),
            'prior/rejected pair retained; verify active references before retirement')
    entries = [{'stage': str(a), 'destination': str(b), 'name': b.name,
                'stagePin': pin(a), 'originalPin': pin(b) if b.exists() else None} for a, b in pairs]
    save(journal, {'schema': 'polaris-mobile-pair-transaction-v1', 'entries': entries})
    try:
        if any(row['originalPin'] is not None for row in entries):
            previous.mkdir(mode=0o700)
        for row in entries:
            stage, destination = Path(row['stage']), Path(row['destination'])
            if row['originalPin'] is not None:
                require(pin(destination) == row['originalPin'], 'old pair changed before commit')
                destination.rename(previous / row['name'])
            stage.rename(destination)
        validate()
        completed = work / 'completed-transaction.json'
        os.replace(journal, completed)
    except BaseException:
        recover(pairs, work)
        raise
