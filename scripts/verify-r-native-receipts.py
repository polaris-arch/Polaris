#!/usr/bin/env python3
"""Pure finite R receipt verification. Parsing is never execution admission.

LogicOnly fixtures remain LogicOnly. Source/ELF/plan flags do not issue authority;
full case0/28 stays ConsumerNotReady without the original consumer-owner adapter.
"""
import argparse
import base64
import collections
import hashlib
import ipaddress
import json
import os
import re
import stat
import struct
from pathlib import Path

PROFILE = "native-tun-rtnetlink-child-userns-v1"
METADATA_SCHEMA = "polaris-g-r-held-input-v1"
ENVELOPE_SCHEMA = "polaris-g-r-case-envelope-v1"
PLAN_SCHEMA = "polaris-g-r-frozen-plan-v1"
MAX_METADATA = 65536
MAX_EVIDENCE = 262144
MAX_STDOUT = 16777216
BARRIER = b"B_NEW_REFUSED\n"
# Exact frozen 5-top/28-sub inventory; no regex or extra Profile/old TestGoKernel admission.
GROUPS = (('TestRNativeT0Construction', ('same_ns_running_a_failed_b_old_callback', 'new_without_start_close', 'started_dispose_construction_double_close')), ('TestRNativeT4Selectors', ('ipv4_default_nil_dst', 'ipv6_default_nil_dst', 'ordinary_full_selectors', 'strict_full_selectors', 'marked_full_selectors', 'foreign_priority_early_refusal', 'occupied_table_early_refusal', 'foreign_metric_preserved', 'wrong_family_preserved', 'wrong_gateway_preserved', 'wrong_link_preserved', 'absent_owned_unknown')), ('TestRNativeT4Borrowed', ('persistent_external_address_preserved', 'borrowed_mtu_up_restored', 'provided_fd_duplicate_only_close', 'multiqueue_transfer_receiver_join')), ('TestRNativeT4Update', ('include_exclude_gateway_success', 'real_eexist_or_explicit_seam_partial_seal', 'admitted_update_close_writer_join')), ('TestRNativeT5OrdinaryTun', ('ipv4_autoroute_on', 'ipv4_autoroute_off', 'ipv6_autoroute_on', 'ipv6_autoroute_off', 'route_exclude_loopback_update', 'private_udp_nonce_tun_read_write')))
CASES = tuple((top, sub) for top, subs in GROUPS for sub in subs)
TOP_COUNTS = (3, 12, 4, 3, 6)
HASH = re.compile(r"[0-9a-f]{64}\Z")
NONCE = re.compile(r"[0-9a-f]{32}\Z")
COMMIT = re.compile(r"[0-9a-f]{40}\Z")
METADATA_FIELDS = frozenset("schema version profile profileSha256 batchNonce caseNonce caseID actor sourceCommit sourceTree sourceFilesSha256 moduleGraphSha256 elfSha256 configSha256 planSha256 fdRoles fdObservedIdentities guardianParentIdentities caseOptions topology budgets".split())
BUDGETS = {"setupSeconds": 10, "caseMilliseconds": 15000, "cleanupSeconds": 5,
           "ioMaximumMilliseconds": 1000, "caseEvidenceMaximumBytes": MAX_EVIDENCE,
           "casePacketsMaximum": 8, "stdoutMaximumBytes": MAX_STDOUT}
FD_ROLES = {3: "targetNetns", 4: "childUserns", 5: "privateMountns", 6: "privateProcDir",
            7: "formalClaimsBaseDir", 8: "readOnlyRegularMetadata", 9: "providedPrivateTun",
            10: "oneShotGToAReadPipe"}
HOST_COMMANDS = (["/usr/sbin/ip", "-j", "link", "show"], ["/usr/sbin/ip", "-j", "address", "show"],
                 ["/usr/sbin/ip", "-j", "-4", "route", "show", "table", "all"],
                 ["/usr/sbin/ip", "-j", "-6", "route", "show", "table", "all"],
                 ["/usr/sbin/ip", "-j", "-4", "rule", "show"], ["/usr/sbin/ip", "-j", "-6", "rule", "show"],
                 ["/usr/sbin/nft", "--json", "list", "ruleset"], ["/usr/sbin/iptables-save"], ["/usr/sbin/ip6tables-save"])
_HARNESS = None


def harness_source():
    # Import contains definitions only. No guardian, CLI, native or fixture is invoked.
    global _HARNESS
    if _HARNESS is None:
        import importlib.util
        path = Path(__file__).with_name("native-netns-harness.py")
        if not path.is_file(): path = Path(__file__).with_name("worker.py")
        spec = importlib.util.spec_from_file_location("r_snapshot_harness", path)
        _HARNESS = importlib.util.module_from_spec(spec); spec.loader.exec_module(_HARNESS)
    return _HARNESS


def check(condition, message):
    if not condition:
        raise ValueError(message)


def exact(value, fields, label):
    check(type(value) is dict and set(value) == set(fields), label + " fields differ")
    return value


def integer(value, lower, upper, label):
    check(type(value) is int and lower <= value <= upper, label + " is outside bounds")
    return value


def closed_json(data, maximum=MAX_METADATA):
    check(type(data) is bytes and 0 < len(data) <= maximum, "JSON byte budget exceeded")
    def unique(pairs):
        result = {}
        for key, value in pairs:
            check(key not in result, "duplicate JSON key")
            result[key] = value
        return result
    def reject_constant(value):
        raise ValueError("non-finite JSON number")
    return json.loads(data.decode("utf-8"), object_pairs_hook=unique, parse_constant=reject_constant)


def encoded(value, maximum=MAX_METADATA):
    data = json.dumps(value, sort_keys=True, separators=(",", ":"), allow_nan=False).encode()
    check(len(data) <= maximum, "encoded evidence byte budget exceeded")
    return data


def role_fds(index, actor):
    integer(index, 0, 27, "caseID")
    check(actor in (("a", "b") if index == 0 else ("single",)), "actor/case differs")
    return tuple(range(3, 9)) + ((9,) if index == 17 else ()) + ((10,) if index == 0 and actor == "a" else ())


def selector(index):
    integer(index, 0, 27, "caseID")
    top, sub = CASES[index]
    return "^" + re.escape(top) + "$/^" + re.escape(sub) + "$"


def barrier_bytes(chunks, eof):
    # Pure codec for the one bounded nonblocking pipe; the actual worker requires B Wait first.
    data = b""
    for chunk in chunks:
        check(type(chunk) is bytes and 0 < len(chunk) <= len(BARRIER), "invalid barrier chunk")
        data += chunk
        check(len(data) <= len(BARRIER) and BARRIER.startswith(data), "unexpected barrier bytes")
    check(eof is True and data == BARRIER, "truncated or nonterminal barrier")
    return data


def validate_metadata(value):
    exact(value, METADATA_FIELDS, "metadata")
    check(value["schema"] == METADATA_SCHEMA and type(value["version"]) is int and value["version"] == 1 and value["profile"] == PROFILE, "metadata identity differs")
    index = integer(value["caseID"], 0, 27, "caseID")
    slots = role_fds(index, value["actor"])
    for key in ("profileSha256", "sourceFilesSha256", "moduleGraphSha256", "elfSha256", "configSha256", "planSha256"):
        check(type(value[key]) is str and HASH.fullmatch(value[key]), key + " differs")
    for key in ("batchNonce", "caseNonce"):
        check(type(value[key]) is str and NONCE.fullmatch(value[key]), key + " differs")
    for key in ("sourceCommit", "sourceTree"):
        check(type(value[key]) is str and COMMIT.fullmatch(value[key]), key + " differs")
    roles = {str(fd): FD_ROLES[fd] for fd in slots}
    check(value["fdRoles"] == roles, "FD roles are not closed")
    exact(value["fdObservedIdentities"], roles, "FD observations")
    for fd in slots:
        item = exact(value["fdObservedIdentities"][str(fd)], {"dev", "ino", "type", "access"}, "FD identity")
        integer(item["dev"], 0, 2**64-1, "FD dev"); integer(item["ino"], 1, 2**64-1, "FD ino")
        check(item["type"] == ({3:"nsfs",4:"nsfs",5:"nsfs",6:"directory",7:"directory",8:"regular",9:"tun",10:"pipe"}[fd]), "FD type differs")
        check(item["access"] == ("readWrite" if fd == 9 else "readOnly"), "FD access differs")
    exact(value["guardianParentIdentities"], {"net", "mnt", "user", "pid", "ipc"}, "guardian identities")
    for item in value["guardianParentIdentities"].values():
        exact(item, {"dev", "ino"}, "guardian identity")
        integer(item["dev"],0,2**64-1,"guardian dev");integer(item["ino"],1,2**64-1,"guardian ino")
    check(value["budgets"] == BUDGETS and all(type(v) is int for v in value["budgets"].values()), "case budgets differ")
    # PC owns the complete actual Options/topology dictionary; missing source ACK is NotReady.
    try:
        harness_source().r_options(value["caseOptions"], index, value["actor"])
        harness_source().r_topology(value["topology"], index)
    except RuntimeError as error:
        raise ValueError(str(error)) from error
    encoded(value)
    return value


def go_transcript(data, index, exit_status):
    check(type(data) is bytes and len(data) <= MAX_STDOUT, "child stream bound")
    top, sub = CASES[index]; full = top + "/" + sub
    starts, terminals = [], collections.defaultdict(list)
    for line in data.decode("utf-8").splitlines():
        if line.startswith("=== RUN   "):
            starts.append(line[len("=== RUN   "):])
        match = re.fullmatch(r"\s*--- (PASS|FAIL|SKIP): ([A-Za-z0-9_/]+) \([0-9.]+s\)", line)
        if match: terminals[match[2]].append(match[1])
    check(type(exit_status) is int and exit_status == 0 and starts == [top,full] and dict(terminals) == {full:["PASS"],top:["PASS"]}, "missing/extra/skip/failed Go result")
    return True


def netlink_dump(value):
    # Independently decode each original recv datagram; no mutation wire ACK assertion.
    exact(value, {"request","sequence","localPortID","receives","terminalStatus","decoded"}, "raw dump")
    request = integer(value["request"], 0, 255, "request")
    expected = {18:16,22:20,26:24,34:32}
    check(request in expected, "non-GET raw request")
    seq=integer(value["sequence"],1,2**32-1,"sequence");local=integer(value["localPortID"],1,2**32-1,"local port")
    check(type(value["receives"]) is list and 1 <= len(value["receives"]) <= 256, "raw recv budget")
    decoded=[];done=False;total=0
    for recv in value["receives"]:
        exact(recv, {"senderPortID","senderGroups","recvFlags","receivedLength","rawDatagrams"}, "raw recv")
        check(type(recv["senderPortID"]) is int and recv["senderPortID"] == 0 and type(recv["senderGroups"]) is int and recv["senderGroups"] == 0 and type(recv["recvFlags"]) is int and recv["recvFlags"] == 0, "nonkernel/truncated raw recv")
        check(type(recv["rawDatagrams"]) is str, "raw datagram encoding")
        data=base64.b64decode(recv["rawDatagrams"],validate=True);total+=len(data)
        check(type(recv["receivedLength"]) is int and recv["receivedLength"] == len(data) and total <= MAX_EVIDENCE, "raw received length/budget differs")
        pos=0
        while pos < len(data):
            check(not done and len(data)-pos >= 16, "raw trailing or message after DONE")
            length,kind,flags,msgseq,pid=struct.unpack_from("<IHHII",data,pos)
            check(16<=length<=len(data)-pos and msgseq==seq and pid==local and not flags & 0x10, "raw header/seq/port/DUMP_INTR")
            body=data[pos+16:pos+length]
            if kind==3:
                check(body == b"\0\0\0\0", "missing/nonzero DONE error");done=True
            else:
                check(kind==expected[request] and flags & 2, "raw error/overrun/nonmultipart/unexpected message")
                fixed={16:16,20:8,24:12,32:12}[kind];check(len(body)>=fixed,"truncated RTM payload")
                attrs={};offset=fixed
                while offset<len(body):
                    check(len(body)-offset>=4,"truncated RTM attribute")
                    size,atype=struct.unpack_from("<HH",body,offset)
                    # Preserve type flags and every original attribute byte. Nested payloads are
                    # bounded opaque bytes, not silently discarded or promoted to decoded facts.
                    check(4<=size<=len(body)-offset and atype not in attrs,"duplicate/invalid RTM attribute")
                    attrs[atype]=body[offset+4:offset+size].hex();offset+=(size+3)&~3
                    check(offset<=len(body),"RTM alignment exceeds payload")
                # Full fixed selector bytes and all attributes retained, not a guessed reduced spec.
                decoded.append({"kind":kind,"header":body[:fixed].hex(),"attributes":{str(k):v for k,v in sorted(attrs.items())}})
            pos+=(length+3)&~3;check(pos<=len(data),"NLMSG alignment exceeds datagram")
    check(done and value["terminalStatus"]=="DONE" and value["decoded"]==decoded,"missing DONE or independently decoded spec differs")
    return decoded


def verify_actor(value, index, actor, binding, planned=None, producer_list=None):
    exact(value, {"actor","metadata","pid","wait","stdout","stderr","list","evidence"}, "actor envelope")
    check(value["actor"]==actor,"actor role differs");metadata=validate_metadata(value["metadata"])
    check(metadata["caseID"]==index and metadata["actor"]==actor,"actor metadata differs")
    for key in binding:check(metadata[key]==binding[key],"candidate/nonce/plan mismatch: "+key)
    integer(value["pid"],1,2**31-1,"actual child pid")
    wait=exact(value["wait"],{"pid","exitStatus","actual","timedOut"},"actual Wait")
    check(wait["pid"]==value["pid"] and wait["actual"] is True and wait["timedOut"] is False,"missing actual Wait")
    item=exact(value["list"],{"top","argv","stdout","exitStatus","elfSha256"},"producer top list")
    top=CASES[index][0]
    check(item=={"top":top,"argv":["-test.list=^"+top+"$"],"stdout":top+"\n","exitStatus":0,"elfSha256":binding["elfSha256"]} and type(item["exitStatus"]) is int,"selected top list differs")
    if producer_list is not None:check(item==producer_list,"producer actual selected-list plan splice")
    if planned is not None:
        check(metadata["caseOptions"]==planned["options"][actor] and metadata["topology"]==planned["topology"],"frozen actor Options/topology splice")
    stdout=base64.b64decode(value["stdout"],validate=True);stderr=base64.b64decode(value["stderr"],validate=True)
    check(len(stdout)+len(stderr)<=MAX_STDOUT,"combined child output budget")
    go_transcript(stdout,index,wait["exitStatus"])
    # These exact typed records must be emitted by the PC's actual producer. Missing fields reject.
    evidence=value["evidence"];check(type(evidence) is list and 2<=len(evidence)<=256,"typed evidence absent/budget")
    emitted=[closed_json(line[len(b"R_NATIVE_EVIDENCE "):],MAX_EVIDENCE) for line in stdout.splitlines() if line.startswith(b"R_NATIVE_EVIDENCE ")]
    check(emitted==evidence,"typed evidence does not equal original child stdout bytes")
    phases=[];packets=0;birth=None
    for event in evidence:
        exact(event,{"schema","phase","batchNonce","caseNonce","caseID","actor","birth","options","facts","packets","rawDumps","subjectCleanup","fixtureDisposal"},"native evidence")
        check(event["schema"]=="polaris-r-native-evidence-v1" and event["phase"] in ("pending","running","priority_refused","complete"),"unknown evidence phase")
        for key in ("batchNonce","caseNonce","caseID","actor"):check(event[key]==metadata[key],"native evidence association differs")
        check(type(event["birth"]) is str and NONCE.fullmatch(event["birth"]),"actual native birth absent")
        if birth is None:birth=event["birth"]
        check(event["birth"]==birth,"actor birth changed mid-receipt")
        check(event["options"]==metadata["caseOptions"],"actual Options receipt differs")
        integer(event["packets"],0,8,"packet count");packets+=event["packets"]
        check(type(event["rawDumps"]) is list and len(event["rawDumps"])<=16,"raw dump budget")
        for raw in event["rawDumps"]:netlink_dump(raw)
        check(type(event["facts"]) is dict,"typed original facts missing")
        check(event["subjectCleanup"] in ("Pending","Closed","ConstructionOnlyClosed","Unknown","PartialSealed") and event["fixtureDisposal"] in ("Pending","ActualRestoredAndDisposed","Unknown"),"unknown typed disposition")
        phases.append(event["phase"])
    check(packets<=metadata["topology"]["packetBudgetAllocation"]["native"] and phases[0]=="pending" and phases[-1]=="complete" and phases.count("pending")==1 and phases.count("complete")==1,"phase/packet closure differs")
    check(sum(len(encoded(x,MAX_EVIDENCE)) for x in evidence)<=MAX_EVIDENCE,"cumulative case evidence exceeds bound")
    if planned is not None:
        check(evidence[-1]["subjectCleanup"]==planned["expectedSubjectCleanup"] and evidence[-1]["fixtureDisposal"]==planned["expectedFixtureDisposal"],"planned terminal disposition differs")
    return evidence


def verify_batch(envelopes, plan=None, plan_bytes=None):
    check(type(envelopes) is list and len(envelopes)==28,"exact28 case envelopes required")
    indexes=[x.get("caseID") for x in envelopes];check(sorted(indexes)==list(range(28)),"missing/duplicate/extra case")
    nonces=set();common=None;counts=collections.Counter();logic_only=False
    if plan is not None:
        from types import SimpleNamespace
        try:harness_source().r_plan(plan, SimpleNamespace(**globals()))
        except RuntimeError as error:raise ValueError(str(error)) from error
        check(type(plan_bytes) is bytes and closed_json(plan_bytes,4194304)==plan,"actual frozen plan bytes differ")
    columns={"testAssertions":"VerifiedInEnvelope","rCalleeCoverage":"NotReady","consumerCoverage":"NotReady",
             "subjectCleanup":[],"fixtureDisposal":[],"guardianSettlement":"VerifiedInEnvelope","hostUnchanged":"VerifiedInEnvelope","aggregateResult":"ConsumerNotReady"}
    for envelope in sorted(envelopes,key=lambda x:x["caseID"]):
        exact(envelope,{"schema","caseID","selected","binding","actors","barrier","settlement","host","fixture","evidenceClass"},"case envelope")
        index=integer(envelope["caseID"],0,27,"caseID");check(envelope["schema"]==ENVELOPE_SCHEMA and envelope["selected"]=="/".join(CASES[index]),"case selector differs")
        binding=exact(envelope["binding"],{"profileSha256","batchNonce","caseNonce","sourceCommit","sourceTree","sourceFilesSha256","moduleGraphSha256","elfSha256","configSha256","planSha256"},"binding")
        if plan is not None:
            expected={key:plan[key] for key in binding if key not in ("caseNonce","planSha256")}
            expected.update(caseNonce=plan["caseNonces"][index],planSha256=hashlib.sha256(plan_bytes).hexdigest())
            check(binding==expected,"actual frozen plan/candidate binding differs")
        check(binding["caseNonce"] not in nonces,"reused case nonce");nonces.add(binding["caseNonce"])
        candidate={k:v for k,v in binding.items() if k!="caseNonce"}
        if common is None:common=candidate
        check(candidate==common,"cross-run candidate/plan splice")
        roles=("a","b") if index==0 else ("single",)
        check(type(envelope["actors"]) is list and [a["actor"] for a in envelope["actors"]]==list(roles),"actor count/order differs")
        planned=None if plan is None else plan["cases"][index]
        producer=None if plan is None else next(x for x in plan["selectedTopLists"] if x["top"]==CASES[index][0])
        events=[verify_actor(a,index,role,binding,planned,producer) for a,role in zip(envelope["actors"],roles)]
        check(sum(len(encoded(e,MAX_EVIDENCE)) for actor in events for e in actor)<=MAX_EVIDENCE and sum(e["packets"] for actor in events for e in actor)<=8,"combined actor evidence/packet budget")
        check(sum(len(base64.b64decode(a[s],validate=True)) for a in envelope["actors"] for s in ("stdout","stderr"))<=MAX_STDOUT,"combined actor stdout/stderr budget")
        if index==0:
            barrier=exact(envelope["barrier"],{"bytes","aReadyBeforeBStart","bWaitBeforeWrite","closedAfterWrite","aAliveAtWrite"},"A/B barrier")
            check(base64.b64decode(barrier["bytes"],validate=True)==BARRIER and all(barrier[k] is True for k in barrier if k!="bytes"),"A/B actual Wait barrier absent")
            a,b=events;check(any(e["phase"]=="running" and e["facts"].get("phaseLabel")=="independent_a_running_before_b" for e in a),"actual A not observed ready")
            refusal=next((e for e in b if e["phase"]=="priority_refused"),None)
            check(refusal is not None and refusal["facts"]=={"newReturnedNil":True,"errno":"EEXIST","source":"priority_preflight","tunOpenCount":0,"startCount":0,"callbackCount":0},"B actual refusal differs")
            check(a[0]["birth"]!=b[0]["birth"],"A/B birth equal")
        else:check(envelope["barrier"] is None,"unexpected actor control")
        settlement=exact(envelope["settlement"],{"allChildrenWaited","controlledNamespaceLifetimeEnded","cleanupWithinMillis"},"guardian settlement")
        check(settlement["allChildrenWaited"] is True and settlement["controlledNamespaceLifetimeEnded"] is True,"unsettled guardian")
        integer(settlement["cleanupWithinMillis"],0,5000,"cleanup budget")
        fixture=exact(envelope["fixture"],{"restored","fixtureDisposal","persistentBefore","persistentAfter","writerBefore","writerAfter","providedOriginalIdentity","disposedObjects","protectedMetadataUnchanged","setupLeavesRestored"},"G fixture disposition")
        check(fixture["restored"] is True and fixture["fixtureDisposal"]=="ActualRestoredAndDisposed" and fixture["protectedMetadataUnchanged"] is True and fixture["setupLeavesRestored"] is True,"G fixture disposition unknown")
        check((fixture["persistentBefore"] is not None)==(index in (15,16,17)) and fixture["persistentBefore"]==fixture["persistentAfter"],"G actual persistent restoration differs")
        if index==16:
            before=exact(fixture["writerBefore"],{"path","identity","value"},"G writer before")
            after=exact(fixture["writerAfter"],{"identity","value"},"G writer after")
            check(before["path"]=="/proc/sys/net/ipv4/conf/rnt16/rp_filter" and type(before["identity"]) is int and before["identity"]>0 and after["identity"]==before["identity"] and before["value"]==after["value"]=="1\n","G held writer leaf restoration differs")
        else:check(fixture["writerBefore"] is None and fixture["writerAfter"] is None,"unexpected G writer")
        check((fixture["providedOriginalIdentity"] is not None)==(index==17),"G original provided FD differs")
        if index==17:check(fixture["providedOriginalIdentity"]=={k:envelope["actors"][0]["metadata"]["fdObservedIdentities"]["9"][k] for k in ("dev","ino")},"provided FD original identity splice")
        expected_names=( ["rnt%02d"%index] if index in (15,16,17) else ["rfg%02d"%index] if index in (11,12,13,19) else [] )
        check(type(fixture["disposedObjects"]) is list and [x.get("name") for x in fixture["disposedObjects"]]==expected_names and all(type(x.get("ifindex")) is int and x["ifindex"]>0 and set(x)=={"name","ifindex"} for x in fixture["disposedObjects"]),"G-created fixture disposal identities differ")
        host=exact(envelope["host"],{"before","after","claimsBefore","claimsAfter","namespaceBefore","namespaceAfter"},"host snapshots")
        check(type(host["before"]) is list and type(host["after"]) is list and [x.get("command") for x in host["before"]]==list(HOST_COMMANDS) and [x.get("command") for x in host["after"]]==list(HOST_COMMANDS) and host["claimsBefore"]==host["claimsAfter"] and host["namespaceBefore"]==host["namespaceAfter"],"host claim/namespace/command evidence differs")
        # The comparison uses the original harness normalization, not native flags.
        check(all(type(x.get("status")) is int and x["status"]==0 for x in host["before"]+host["after"]) and harness_source().compare_snapshots(host["before"],host["after"])["equal"],"host raw configuration differs")
        for e in events:
            terminal=e[-1];columns["subjectCleanup"].append(terminal["subjectCleanup"]);columns["fixtureDisposal"].append(terminal["fixtureDisposal"])
            if index==14:check(terminal["subjectCleanup"]=="Unknown","expected Unknown subject laundered")
            if index==20:check(terminal["subjectCleanup"]=="PartialSealed","explicit partial seal laundered")
        check(envelope["evidenceClass"] in ("LogicOnly","ActualNative"),"unknown evidence class")
        check(envelope["evidenceClass"]!="ActualNative" or plan is not None,"actual receipt lacks frozen producer plan")
        logic_only |= envelope["evidenceClass"]=="LogicOnly";counts[CASES[index][0]]+=1
    check(tuple(counts[top] for top,_ in GROUPS)==TOP_COUNTS,"five-top inventory differs")
    if logic_only:
        for key in ("testAssertions","rCalleeCoverage","guardianSettlement","hostUnchanged"):columns[key]="LogicOnly"
        columns["aggregateResult"]="LogicOnly"
    return columns


def main():
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument("--plan",type=Path);parser.add_argument("envelope",nargs=28,type=Path);args=parser.parse_args()
    envelopes=[closed_json(bounded_file(p, 2*MAX_STDOUT+MAX_EVIDENCE+2*MAX_METADATA),2*MAX_STDOUT+MAX_EVIDENCE+2*MAX_METADATA) for p in args.envelope]
    plan_bytes=bounded_file(args.plan,4194304) if args.plan else None
    plan=closed_json(plan_bytes,4194304) if plan_bytes else None
    print(json.dumps(verify_batch(envelopes,plan,plan_bytes),sort_keys=True))
    return 2 # ConsumerNotReady or LogicOnly is never full case0/28 functional PASS.


def bounded_file(path, maximum):
    fd=os.open(path,os.O_RDONLY|os.O_NOFOLLOW|os.O_CLOEXEC)
    try:
        before=os.fstat(fd);check(stat.S_ISREG(before.st_mode) and 0<before.st_size<=maximum,"receipt input type/size")
        data=bytearray()
        while chunk:=os.read(fd,min(65536,maximum+1-len(data))):
            data.extend(chunk);check(len(data)<=maximum,"receipt grew over byte budget")
        after=os.fstat(fd)
        check((before.st_dev,before.st_ino,before.st_size,before.st_mtime_ns)==(after.st_dev,after.st_ino,after.st_size,after.st_mtime_ns) and len(data)==before.st_size,"receipt changed while reading")
        return bytes(data)
    finally:os.close(fd)


if __name__ == "__main__":
    try:raise SystemExit(main())
    except (ValueError,UnicodeError) as error:raise SystemExit("R receipt rejected: "+str(error))
