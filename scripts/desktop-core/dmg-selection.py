"""Technical listing records for the native DMG/HFS/APFS selection reader."""


def member_records(listing, read_link, max_members):
    def require(condition, message):
        if not condition:
            raise ValueError(message)

    require('----------' in listing, 'formal 7-Zip member boundary absent')
    header, body = listing.split('----------', 1)
    # Use the backend's line ending, rather than splitlines(): HFS has a real
    # reserved directory whose name ends in CR. Preserve other control bytes
    # for the existing namespace validator to reject.
    newline = '\r\n' if '\r\n' in header else '\n'
    records = {}
    count = 0
    for block in body.split(newline + newline):
        row = {}
        for line in block.split(newline):
            if not line:
                continue
            require(' = ' in line, 'invalid formal DMG property line')
            key, value = line.split(' = ', 1)
            require(key not in row, 'duplicate formal 7-Zip property')
            row[key] = value
        if not row:
            continue
        count += 1
        require(count <= max_members, 'formal archive member count budget')
        require('Path' in row, 'formal 7-Zip member path absent')
        name = row['Path']
        mode = row.get('Mode', '')
        directory = row.get('Folder') == '+' or mode.startswith('d') or row.get('Attributes', '').startswith('D')
        target = row.get('Symbolic Link') or row.get('Hard Link') or ''
        link = bool(target) or mode.startswith('l')
        require(not (directory and link), 'formal DMG directory/link conflict')
        require(not mode or mode[0] in 'd-l', 'formal DMG nonregular member')
        kind = 'dir' if directory else 'link' if link else 'file'
        size = row.get('Size', '')
        require(size.isdecimal() or not size and kind in ['dir', 'link'], 'formal 7-Zip member size differs')
        # This zero-content filesystem bookkeeping directory is not a payload
        # namespace. Never normalize its CR into a different selectable name.
        if name.rsplit('/', 1)[-1] == '.HFS+ Private Directory Data\r':
            require(kind == 'dir' and size in ['', '0'], 'invalid HFS metadata directory')
            continue
        require(name not in records, 'formal archive duplicate member')
        if kind == 'link' and not target:
            # HFS prints only Mode=l; the bounded native member stream contains
            # the link target. APFS supplies the target as a listing property.
            target = read_link(name, int(size) if size else None).decode('utf-8')
        records[name] = {'kind': kind, 'bytes': int(size or '0'), 'target': target}
    return records


def require_executable_data(selected):
    # HFS can expose a hard-link stub with Size=0 while its bytes reside in the
    # private inode directory, without exposing the association in its listing.
    # Such a stub is not evidence of the executable's actual data fork.
    if any(size is None or size <= 0 for _name, _target, size in selected):
        raise ValueError('formal DMG empty or unresolved hard-linked executable unsupported')
