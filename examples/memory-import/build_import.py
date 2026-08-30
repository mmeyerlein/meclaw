#!/usr/bin/env python3
"""Turn one member export into one import manifest.

A `member` exports what it IS -- since GH #471 that is three documents, not one:
`memory-hive/seed/<table>.jsonl` (what was said to this person), `affinity/…`
(the curated record that decides who may be told what) and `firewall/…` (the
screen every inbound turn passes). Each directory carries its own
`seed/export_final.json` when that hive's walk finished, and one
`export_final.json` beside them names every hive that did.

Getting those sets back IN is where the tree has no declared door, because the
only manifest key that carries FILES is `add_templates[].files` and the shipped
`member` reaches all three holders through a `ref` -- and nothing can put a seed
into a reference.

So this tool writes the references OUT. It reads the shipped `member` and every
hive it references that the export carries, splices them in, drops each hive's
export files under that hive's own store seed directory, and prints ONE manifest
that registers the tree as a local template and instantiates it in the SAME
diff. The order is not a style choice: a seed is read exactly once, when the
`cell.db` is created. A member that is already running cannot be given a past.

Usage:

    build_import.py --export DIR --templates DIR --scope /os/orgs/acme \\
                    --name alex [--template-name member-alex] [--indent 2]

`--export` is the directory the export sink wrote (`params.export_dir` of
`member/export-sink`), the one that HOLDS the per-hive directories -- not one of
them, and not a `seed/` itself. A directory in the pre-#471 flat shape
(`DIR/seed/…` with no per-hive level) is still read, as a memory-hive-only
export: that is what every export written before the sink learned to file by
hive looks like, and refusing it would strand the documents already on disk.

The manifest goes to stdout. Apply it with `meclaw --apply`, or post it to the
mutation door.

Not every hive an export carries can be seeded at BIRTH. `session-keeper` is the
one in the catalogue: it stands inside a generation, and the shipped `member`
ships that generation's container EMPTY -- there is no path in a derived member
template where its seed could live, because the assistant does not exist until
somebody instantiates one. Since GH #475 the member forwards `in_import` to it
all the same, so the document is not stranded, it is just late: `--after-boot`
writes the messages that carry it in once the member is up and its generation is
grown. That is the whole of step 2 for such a hive, written out instead of
described.
"""

import argparse
import json
import os
import sys

# What the member holds as a `ref`, and where each one's seed set lives inside
# it: {hive directory: (store cell, files of the shipped seed that are the
# RECEIVING hive's own configuration and must survive the splice)}.
#
# `emb_models.jsonl` is the only such file in the catalogue: it says which
# embedding generation is live behind which endpoint, the export deliberately
# does not carry it (`memory-hive/README.md`), and an export directory that
# contains one was hand-edited -- merging it would give the new member two live
# generations or none.
#
# Everything ELSE in a shipped seed directory is placeholder content, and it is
# removed wherever this export carries that hive. `affinity` ships a fictional
# person and `firewall` ships eight example rules; leaving them beside an
# imported record would grow a member who knows somebody nobody imported.
PLACEABLE = {
    "memory-hive": ("store", {"emb_models.jsonl"}),
    "affinity": ("store", set()),
    "firewall": ("rules", set()),
}

# The marker the sink writes after the last part of a complete walk. Without it
# the directory is a PREFIX of a document, and a prefix looks exactly like a
# whole one from the outside -- which is the reason the marker exists. The
# member-level file of the same name lists the hives that finished.
MARKER = "export_final.json"

# Files that describe the shipped template to a reader rather than to the
# substrate. They are not copied: a derived template that carries the original's
# prose claims things about itself that are no longer true of it.
SKIP_SUFFIXES = (".md",)


def die(msg):
    sys.stderr.write("build_import: %s\n" % msg)
    raise SystemExit(2)


def read(path):
    with open(path, "r", encoding="utf-8") as fh:
        return fh.read()


def collect(root, prefix=""):
    """Every file below `root`, as {relative path: content}."""
    out = {}
    for entry in sorted(os.listdir(root)):
        full = os.path.join(root, entry)
        rel = prefix + entry
        if os.path.isdir(full):
            out.update(collect(full, rel + "/"))
        elif not rel.endswith(SKIP_SUFFIXES):
            out[rel] = read(full)
    return out


def ref_target(config_text):
    """The template a `{"cell": {"type": "ref"}}` config points at, or None."""
    try:
        cfg = json.loads(config_text)
    except ValueError:
        return None
    cell = cfg.get("cell") or {}
    if cell.get("type") != "ref":
        return None
    return str(cell.get("template") or "").split("@")[0]


def hive_seed_files(seed_dir, own_seed):
    """One hive's seed files, checked for the two ways a set can lie."""
    if not os.path.isfile(os.path.join(seed_dir, MARKER)):
        die("%s carries no %s: the walk did not finish, so this directory is a "
            "PREFIX of a document. A partial record is not a backup, and "
            "nothing in the files themselves would say so" % (seed_dir, MARKER))
    files = {}
    for entry in sorted(os.listdir(seed_dir)):
        if not entry.endswith(".jsonl"):
            continue
        if entry in own_seed:
            die("%s carries %s. That table is the RECEIVING hive's own "
                "configuration and never travels; an export that has one was "
                "edited by hand" % (seed_dir, entry))
        files[entry] = read(os.path.join(seed_dir, entry))
    if not files:
        die("%s carries a marker and no tables" % seed_dir)
    return files


def export_parts(export_dir):
    """Every hive this export carries, as {hive: {table file: content}}.

    Two shapes are read. The one the sink writes since GH #471 is a directory
    per hive; the flat one it wrote before -- `seed/` directly under the export
    directory -- is read as a memory-hive-only export, because that is what
    every document already on disk looks like and refusing it would strand
    them.
    """
    if not os.path.isdir(export_dir):
        die("%s is not a directory" % export_dir)
    flat = os.path.join(export_dir, "seed")
    if os.path.isdir(flat):
        return {"memory-hive": hive_seed_files(flat, PLACEABLE["memory-hive"][1])}, True
    parts = {}
    for entry in sorted(os.listdir(export_dir)):
        seed_dir = os.path.join(export_dir, entry, "seed")
        if not os.path.isdir(seed_dir):
            continue
        own = PLACEABLE.get(entry, (None, set()))[1]
        parts[entry] = hive_seed_files(seed_dir, own)
    if not parts:
        die("%s holds neither a seed/ directory nor one per hive -- point "
            "--export at the sink's export_dir, not at a seed/ itself"
            % export_dir)
    return parts, False


def written_out(templates_root, hive_dirs):
    """The shipped `member`, with each named hive it references spliced in."""
    member_root = os.path.join(templates_root, "member")
    if not os.path.isfile(os.path.join(member_root, "template.json")):
        die("no member template under %s" % templates_root)
    files = collect(member_root)

    spliced = {}
    for hive_dir in hive_dirs:
        marker = hive_dir + "/config.json"
        target = ref_target(files.get(marker, ""))
        if target is None:
            die("%s is not a ref -- this tool splices a reference out, and "
                "there is none here. Read the member template first" % marker)
        hive_root = os.path.join(templates_root, target)
        if not os.path.isfile(os.path.join(hive_root, "config.json")):
            die("member points at template %r, which is not under %s"
                % (target, templates_root))

        # The reference is REPLACED, never kept beside its own expansion: two
        # configs at one path is the ambiguity the substrate refuses to resolve.
        del files[marker]
        # The hive's own template.json does not travel: inside another template
        # a directory is a subtree, and a second template.json there would claim
        # the subtree is a library entry of its own.
        for rel, body in collect(hive_root, hive_dir + "/").items():
            if os.path.basename(rel) != "template.json":
                files[rel] = body
        spliced[hive_dir] = target
    return files, spliced


def derived_template_json(original, name, written):
    meta = json.loads(original)
    return json.dumps({
        "name": name,
        "version": "1.0.0",
        "description": {
            "purpose": "One person's level, derived from %s@%s so that an "
                       "export of what that person IS can be carried in as a "
                       "birth seed. The only structural difference to the "
                       "original is that %s written out instead of referenced "
                       "-- a reference has no files, and a seed is a file."
                       % (meta.get("name"), meta.get("version"), written),
            "use_when": "Once, for the member this export belongs to. A derived "
                        "template is bound to the content it carries and is not "
                        "a library entry: a second member grown from it would be "
                        "born with somebody else's past.",
            "not_in_scope": "Not a substitute for the shipped level. It does not "
                            "follow it: the original moves on, this one keeps "
                            "the shape it was written out of.",
        },
        "tags": sorted(set(meta.get("tags") or []) | {"derived", "import"}),
        "author": meta.get("author"),
        "license": meta.get("license"),
    }, indent=2, ensure_ascii=False) + "\n"


def edges(name):
    """The wiring the shipped level documents, one member into its container.

    Written in the NARROW form since GH #503, and that is the same decision the
    manifest below makes: the declaration stands AT the `members` container, so
    the container is `.` and the member is named bare. The absolute edges are
    byte for byte the ones the wide spelling drew; what moves is the scope root
    the declaration asks the broker for. `templates/builder/recipes`'
    `grow_level` renders exactly this set, and
    `gh470_a_grown_container_level_carries_its_export_lanes.rs` compares the two
    element for element -- so the spelling is not a style choice here either.
    """
    # `in_import` is deliberately not among them, although the shipped level
    # accepts it since member@1.4.0. The org above this container does not carry
    # the lane, so an edge for it here would be one nothing can ever deliver to
    # -- the second step addresses the member path itself.
    inbound = ["in_turn", "in_recall", "in_brief", "in_propose",
               "in_build_result", "in_export"]
    outbound = ["answer", "bundle", "ack", "reject", "error", "write",
                "turn_write", "prune", "build", "close_report", "export_done",
                "pack_ack"]
    # The doors carry the member's own name as well (GH #478). `Edge.to` is a
    # static path, so a container with two members needs two addresses -- and
    # the guard is PERMISSIVE, because nothing promotes `context.member` today:
    # without the key it delivers exactly as it always did, with it, to one
    # member.
    guard = " && (!has(context.member) || context.member == '%s')" % name
    out = [{"from": ".", "to": "./" + name,
            "condition": "has(hop.route) && hop.route == '%s'%s" % (route, guard)}
           for route in inbound]
    out += [{"from": "./" + name, "to": ".",
             "condition": "has(hop.route) && hop.route == '%s'" % route}
            for route in outbound]
    return out


# The hives an export can carry that no member-level template has a path for.
# Each one is late rather than lost: the member forwards `in_import` to it since
# GH #475, so the document goes in as messages once the level is running.
AFTER_BOOT = {
    # hive: the context key that addresses the holder inside the member, or None
    "session-keeper": "assistant",
}


def import_part(hive, marker, table_file, body, index, of):
    """One `in_import` part, rebuilt from what the sink wrote out.

    The sink files a part as a SEED file -- `{"schema": …}` on line 1 and one row
    per line after it -- because that is the shape a store reads at birth. Going
    back the other way is the same document read the other way round: the header
    line is the part's schema, the rest are its rows, and everything that is not
    in the file (the format, the export id, when the walk ran) is in the marker
    the sink wrote beside them.
    """
    lines = [line for line in body.splitlines() if line.strip()]
    if not lines:
        die("%s/%s is empty -- a seed file always carries its schema header"
            % (hive, table_file))
    try:
        header = json.loads(lines[0])
        rows = [json.loads(line) for line in lines[1:]]
    except ValueError as exc:
        die("%s/%s is not a seed file: %s" % (hive, table_file, exc))
    schema = header.get("schema")
    if not isinstance(schema, dict):
        die("%s/%s has no schema header. A row list without one is a guess, and "
            "the receiving porter refuses it as such" % (hive, table_file))
    return {"format": marker.get("format"), "hive_template": hive,
            "export_id": marker.get("export_id"), "exported_at": marker.get("exported_at"),
            "table": table_file[:-len(".jsonl")], "part": index, "of": of,
            "final": index == of, "absent": False, "schema": schema, "rows": rows}


def after_boot_messages(export_dir, target, parts, holders):
    """The messages that carry the late hives in, ready to post at the member.

    One message per part, addressed at the member's own path with
    `hop.import_hive` naming the holder -- the door the member has carried since
    GH #475. Applying the same message twice leaves the same state, so the list
    is a repair procedure as much as a transfer.
    """
    out = []
    for hive in sorted(parts):
        marker_path = os.path.join(export_dir, hive, "seed", MARKER)
        marker = json.loads(read(marker_path)) if os.path.isfile(marker_path) else {}
        table_files = sorted(parts[hive])
        if len(table_files) > 1:
            # The sink writes files, and files carry no walk ORDER. One table is
            # one part and the question does not arise; several would have to be
            # applied in the order the source walked them, which the directory
            # does not record. Saying so beats inventing a sequence.
            die("%s carries %d tables and this tool cannot order them: a hive's "
                "walk order is not recoverable from the files the sink wrote, "
                "and a part marked final in the wrong place re-derives too early"
                % (hive, len(table_files)))
        of = len(table_files)
        for index, table_file in enumerate(table_files, start=1):
            part = import_part(hive, marker, table_file, parts[hive][table_file],
                               index, of)
            hop = {"route": "in_import", "import_hive": hive}
            header = {"hop": hop}
            key = AFTER_BOOT.get(hive)
            if key:
                if not holders.get(key):
                    die("a %s part needs --%s: the member forwards it into the "
                        "container that holds the generations, and which one it "
                        "lands in is decided by context.%s -- a member with two "
                        "generations has two session ledgers, and they are not "
                        "one document" % (hive, key, key))
                header["context"] = {key: holders[key]}
            out.append({"target": target, "header": header,
                        "body": {"messages": [{"origin": "assistant", "type": "text",
                                               "text": json.dumps(part, sort_keys=True)}]}})
    return out


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--export", required=True,
                    help="the sink's export_dir (the directory holding the "
                         "per-hive directories)")
    ap.add_argument("--templates", required=True, help="the template library")
    ap.add_argument("--scope", required=True,
                    help="the org the member is grown into, e.g. /os/orgs/acme "
                         "(the manifest declares itself at its `members` "
                         "container, one storey down)")
    ap.add_argument("--name", required=True, help="the member's name")
    ap.add_argument("--template-name", default=None,
                    help="the local template name (default: member-<name>)")
    ap.add_argument("--indent", type=int, default=2)
    ap.add_argument("--after-boot", default=None, metavar="FILE",
                    help="write the `in_import` messages for every hive this "
                         "export carries that cannot be seeded at birth "
                         "(today: session-keeper). They are posted at the "
                         "member's own path AFTER the manifest was applied and "
                         "the generation was grown")
    ap.add_argument("--assistant", default=None,
                    help="the generation whose session keeper receives the "
                         "sessions part -- context.assistant, the same key a "
                         "turn is addressed with. Required with --after-boot "
                         "when the export carries a session-keeper")
    args = ap.parse_args()

    template_name = args.template_name or ("member-" + args.name)
    parts, flat = export_parts(args.export)

    # A hive the export carries that no member-level template has a path for.
    # `session-keeper` is the one in the catalogue: it stands inside a
    # generation, and the container that holds generations ships EMPTY, so there
    # is nowhere in a derived template for its seed to live. It is LATE rather
    # than lost -- since GH #475 the member forwards `in_import` to it, and
    # --after-boot writes the messages that carry it in. Saying so either way is
    # the whole point: a document that quietly went nowhere reads exactly like
    # one that was never exported.
    late = {h: parts.pop(h) for h in sorted(set(parts) - set(PLACEABLE))}
    for hive in late:
        sys.stderr.write(
            "build_import: %s is in this export and is not seeded at birth: it "
            "stands inside a generation, and the container that holds "
            "generations is empty until one is instantiated. %s\n"
            % (hive,
               "Its `in_import` messages are in %s -- post them at the member "
               "after the generation is grown" % args.after_boot if args.after_boot
               else "Re-run with --after-boot FILE to get the `in_import` "
                    "messages that carry it in once the member is up (GH #475)"))

    # Written BEFORE the manifest half can refuse: an export that carries only a
    # late hive still has a way in, and it is this file. Refusing it for having
    # nothing to seed at birth would strand exactly the document this option
    # exists for.
    if args.after_boot:
        target = args.scope.rstrip("/") + "/members/" + args.name
        msgs = after_boot_messages(args.export, target, late,
                                   {"assistant": args.assistant})
        with open(args.after_boot, "w", encoding="utf-8") as fh:
            json.dump(msgs, fh, indent=args.indent, ensure_ascii=False)
            fh.write("\n")
        sys.stderr.write("build_import: %d after-boot message%s for %s -> %s\n"
                         % (len(msgs), "" if len(msgs) == 1 else "s",
                            ", ".join(sorted(late)) or "nothing", args.after_boot))

    if not parts:
        die("nothing in %s can be placed at a member's birth" % args.export)

    files, spliced = written_out(args.templates, sorted(parts))
    names = sorted(spliced)
    written = ((", ".join(names[:-1]) + " and " + names[-1] + " are")
               if len(names) > 1 else names[0] + " is")
    files["template.json"] = derived_template_json(files["template.json"],
                                                   template_name, written)

    seeded = {}
    for hive_dir, table_files in sorted(parts.items()):
        store_cell, own_seed = PLACEABLE[hive_dir]
        seed_prefix = "%s/%s/seed/" % (hive_dir, store_cell)
        # The shipped placeholder rows go: this hive's content is the export's
        # now, and a placeholder that survived it would be a second person in
        # the record. What the RECEIVING hive configures for itself stays.
        for rel in [k for k in files
                    if k.startswith(seed_prefix)
                    and os.path.basename(k) not in own_seed]:
            del files[rel]
        for table_file, body in table_files.items():
            files[seed_prefix + table_file] = body
        seeded[hive_dir] = len(table_files)

    # The declaration stands at the `members` container of the org `--scope`
    # names (GH #503), which is where its edges are relative to and the whole of
    # what it changes. `add_templates` is unaffected either way: a class enters
    # the library under `{templates_root}/local/`, which no scope addresses.
    manifest = {"manifest": [{
        "scope": args.scope.rstrip("/") + "/members",
        "diff": {
            "add_templates": [{"name": template_name, "files": files}],
            "add_nodes": [{"name": args.name,
                           "template": template_name + "@1.0.0"}],
            "add_edges": edges(args.name),
        },
    }]}
    json.dump(manifest, sys.stdout, indent=args.indent, ensure_ascii=False,
              sort_keys=False)
    sys.stdout.write("\n")
    sys.stderr.write(
        "build_import: %s from member + %s, %d files, %s%s\n"
        % (template_name, ", ".join(sorted(set(spliced.values()))), len(files),
           ", ".join("%s: %d table%s" % (h, n, "" if n == 1 else "s")
                     for h, n in sorted(seeded.items())),
           " (flat pre-#471 export directory)" if flat else ""))


if __name__ == "__main__":
    main()
