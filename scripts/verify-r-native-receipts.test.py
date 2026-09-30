#!/usr/bin/env python3
"""LogicOnly finite fixtures. No child, FD, socket, ioctl, namespace or guardian is run."""
import base64
import copy
import hashlib
import importlib.util
import json
from pathlib import Path
import struct
import unittest
from unittest.mock import patch

spec=importlib.util.spec_from_file_location("r_receipts",Path(__file__).with_name("verify-r-native-receipts.py"))
v=importlib.util.module_from_spec(spec);spec.loader.exec_module(v)
h=v.harness_source()


def options(index,actor):
    family=6 if index in (4,24,25) else 4
    return {"name":"rnt00b" if actor=="b" else "rnt%02d"%index,"netnsFD":3,"providedTunFD":9 if index==17 else None,
            "table":40100 if actor=="b" else 40000+index,"rulePriority":12000+64*index,"fallbackPriority":16000+64*index,
            "mtu":1400,"family":family,"autoRoute":index not in (23,25),"strict":index==6,"marked":index==7,"multiQueue":index==18,
            "dnsMode":"disabled","gso":False,"txChecksumOffload":False,"inet4Address":[] if family==6 else ["198.18.0.1/24"],
            "inet6Address":["fd00:727::1/64"] if family==6 else [],"inputMark":0x210001,"outputMark":0x210002,
            "include":[],"exclude":[],"loopback":[],"gateway":None,"actualOptions":{"LogicOnly":True},"actualOptionsSha256":hashlib.sha256(v.encoded({"LogicOnly":True})).hexdigest()}


def topology(index):
    # Deliberately synthetic values, never PC actual config or issued metadata.
    persistent={"name":"rnt%02d"%index,"mtu":1300,"up":index!=16,"addresses":["198.18.42.1/24"]} if index in (15,16,17) else None
    dummy={"name":"rfg%02d"%index,"kind":"dummy","mtu":1400,"up":True,"addresses":["198.18.43.1/24"],
           "route":{"destination":"198.18.44.0/24","gateway":"198.18.43.2","table":40000+index,"metric":1}}
    return {"persistentBefore":persistent,"providedEndpoint":{"destination":"198.18.99.10/32","udpPort":19001,"linkName":"rnt17"} if index==17 else None,
            "foreignLink":dummy if index in (11,12,13) else None,"gatewayLink":dummy if index==19 else None,
            "ipv6OutputPriority":4242 if index==25 else None,"packetBudgetAllocation":{"native":8,"guardian":0}}


def plan_fixture():
    manifest={"LogicOnly/synthetic.go":"2"*64};graph={"complete":True,"modules":[{"path":"LogicOnly","version":"synthetic","replace":None}]}
    plan={key:"3"*64 for key in ("harnessSha256","verifierSha256","profileSha256","configSha256","elfSha256","goModSha256","goSumSha256")}
    plan.update(schema=v.PLAN_SCHEMA,version=1,profile=v.PROFILE,sourceCommit="4"*40,sourceTree="5"*40,
                sourceManifest=manifest,sourceFilesSha256=hashlib.sha256(v.encoded(manifest)).hexdigest(),moduleGraph=graph,moduleGraphSha256=hashlib.sha256(v.encoded(graph)).hexdigest(),
                toolchain="LogicOnly-Go",buildFlags=["polaris_r_native"],batchNonce="6"*32,caseNonces=["%032x"%(index+1) for index in range(28)],
                cases=[],selectedTopLists=[],pcMetadataAck="G_PC_EXACT_DICT_PENDING")
    for index,(top,sub) in enumerate(v.CASES):
        actors=["a","b"] if index==0 else ["single"]
        plan["cases"].append({"selected":top+"/"+sub,"actors":actors,"options":{actor:options(index,actor) for actor in actors},"topology":topology(index),
                              "expectedSubjectCleanup":"Unknown" if index==14 else "PartialSealed" if index==20 else "ConstructionOnlyClosed" if index==1 else "Closed",
                              "expectedFixtureDisposal":"ActualRestoredAndDisposed"})
    plan["selectedTopLists"]=[{"top":top,"argv":["-test.list=^"+top+"$"],"stdout":top+"\n","exitStatus":0,"elfSha256":plan["elfSha256"]} for top,_ in v.GROUPS]
    return plan


def metadata(plan,index,actor,plan_bytes):
    binding={key:plan[key] for key in ("profileSha256","batchNonce","sourceCommit","sourceTree","sourceFilesSha256","moduleGraphSha256","elfSha256","configSha256")}
    binding.update(caseNonce=plan["caseNonces"][index],planSha256=hashlib.sha256(plan_bytes).hexdigest())
    roles={str(fd):v.FD_ROLES[fd] for fd in v.role_fds(index,actor)}
    types={3:"nsfs",4:"nsfs",5:"nsfs",6:"directory",7:"directory",8:"regular",9:"tun",10:"pipe"}
    return {"schema":v.METADATA_SCHEMA,"version":1,"profile":v.PROFILE,**binding,"caseID":index,"actor":actor,"fdRoles":roles,
            "fdObservedIdentities":{str(fd):{"dev":1,"ino":100+fd,"type":types[fd],"access":"readWrite" if fd==9 else "readOnly"} for fd in v.role_fds(index,actor)},
            "guardianParentIdentities":{name:{"dev":1,"ino":i+1} for i,name in enumerate(("net","mnt","user","pid","ipc"))},
            "caseOptions":plan["cases"][index]["options"][actor],"topology":plan["cases"][index]["topology"],"budgets":copy.deepcopy(v.BUDGETS)}


def event(meta,phase,birth,facts=None):
    return {"schema":"polaris-r-native-evidence-v1","phase":phase,**{key:meta[key] for key in ("batchNonce","caseNonce","caseID","actor")},
            "birth":birth,"options":meta["caseOptions"],"facts":facts or {},"packets":0,"rawDumps":[],"subjectCleanup":"Pending","fixtureDisposal":"Pending"}


def actor_fixture(plan,index,actor,plan_bytes):
    meta=metadata(plan,index,actor,plan_bytes);birth="%032x"%(1000+2*index+(actor=="b"))
    events=[event(meta,"pending",birth)]
    if index==0:
        facts={"phaseLabel":"independent_a_running_before_b"} if actor=="a" else {"newReturnedNil":True,"errno":"EEXIST","source":"priority_preflight","tunOpenCount":0,"startCount":0,"callbackCount":0}
        events.append(event(meta,"running" if actor=="a" else "priority_refused",birth,facts))
    terminal=event(meta,"complete",birth);terminal["subjectCleanup"]=plan["cases"][index]["expectedSubjectCleanup"];terminal["fixtureDisposal"]="ActualRestoredAndDisposed";events.append(terminal)
    result={"actor":actor,"metadata":meta,"pid":2000+2*index+(actor=="b"),"wait":{"pid":2000+2*index+(actor=="b"),"exitStatus":0,"actual":True,"timedOut":False},
            "stderr":"","list":next(x for x in plan["selectedTopLists"] if x["top"]==v.CASES[index][0]),"evidence":events}
    restream(result,index)
    return result


def restream(actor,index):
    top,sub=v.CASES[index]
    data=("=== RUN   "+top+"\n=== RUN   "+top+"/"+sub+"\n").encode()
    data+=b"".join(b"R_NATIVE_EVIDENCE "+v.encoded(e,v.MAX_EVIDENCE)+b"\n" for e in actor["evidence"])
    data+=("--- PASS: "+top+"/"+sub+" (0.01s)\n--- PASS: "+top+" (0.01s)\nPASS\n").encode()
    actor["stdout"]=base64.b64encode(data).decode()


def batch_fixture():
    plan=plan_fixture();plan_bytes=v.encoded(plan,4194304);envelopes=[]
    for index,(top,sub) in enumerate(v.CASES):
        actors=[actor_fixture(plan,index,actor,plan_bytes) for actor in plan["cases"][index]["actors"]]
        binding={key:actors[0]["metadata"][key] for key in ("profileSha256","batchNonce","caseNonce","sourceCommit","sourceTree","sourceFilesSha256","moduleGraphSha256","elfSha256","configSha256","planSha256")}
        snapshots=[{"command":command,"status":0,"stdout":"[]" if command[0].endswith("ip") else "{}" if command[0].endswith("nft") else "LogicOnly-empty", "startedMonotonic":100,"finishedMonotonic":101,"stderr":""} for command in v.HOST_COMMANDS]
        persistent={"ifindex":77,"name":"rnt%02d"%index,"mtu":1300,"up":index!=16,"addresses":[{"local":"198.18.42.1","prefixlen":24}]} if index in (15,16,17) else None
        fixture={"restored":True,"fixtureDisposal":"ActualRestoredAndDisposed","persistentBefore":persistent,"persistentAfter":copy.deepcopy(persistent),
                 "writerBefore":{"path":"/proc/sys/net/ipv4/conf/rnt16/rp_filter","identity":88,"value":"1\n"} if index==16 else None,
                 "writerAfter":{"identity":88,"value":"1\n"} if index==16 else None,"providedOriginalIdentity":{"dev":1,"ino":109} if index==17 else None,
                 "disposedObjects":[{"name":"rnt%02d"%index,"ifindex":77}] if index in (15,16,17) else [{"name":"rfg%02d"%index,"ifindex":77}] if index in (11,12,13,19) else [],
                 "protectedMetadataUnchanged":True,"setupLeavesRestored":True}
        envelopes.append({"schema":v.ENVELOPE_SCHEMA,"caseID":index,"selected":top+"/"+sub,"binding":binding,"actors":actors,
                          "barrier":{"bytes":base64.b64encode(v.BARRIER).decode(),"aReadyBeforeBStart":True,"bWaitBeforeWrite":True,"closedAfterWrite":True,"aAliveAtWrite":True} if index==0 else None,
                          "settlement":{"allChildrenWaited":True,"controlledNamespaceLifetimeEnded":True,"cleanupWithinMillis":1},"fixture":fixture,
                          "host":{"before":snapshots,"after":copy.deepcopy(snapshots),"claimsBefore":{"absent":True},"claimsAfter":{"absent":True},"namespaceBefore":{"LogicOnly":1},"namespaceAfter":{"LogicOnly":1}},"evidenceClass":"LogicOnly"})
    return envelopes,plan,plan_bytes


def raw_fixture():
    attr=struct.pack("<HH4s",8,15,b"\x40\x9c\x00\x00")
    body=struct.pack("<BBBBBBBBI",2,24,0,0,0,4,0,1,0)+attr
    raw=struct.pack("<IHHII",16+len(body),24,2,7,9)+body+struct.pack("<IHHIIi",20,3,2,7,9,0)
    value={"request":26,"sequence":7,"localPortID":9,"receives":[{"senderPortID":0,"senderGroups":0,"recvFlags":0,"receivedLength":len(raw),"rawDatagrams":base64.b64encode(raw).decode()}],"terminalStatus":"DONE",
           "decoded":[{"kind":24,"header":body[:12].hex(),"attributes":{"15":body[16:].hex()}}]}
    return value,raw


class CodecTests(unittest.TestCase):
    def test_exact_finite_inventory_and_role_closure(self):
        self.assertEqual(tuple(len(subs) for _,subs in v.GROUPS),(3,12,4,3,6));self.assertEqual(len(v.CASES),28)
        self.assertEqual(sum(len(["a","b"] if i==0 else ["single"]) for i in range(28)),29)
        self.assertEqual(v.role_fds(0,"a"),(3,4,5,6,7,8,10));self.assertEqual(v.role_fds(0,"b"),(3,4,5,6,7,8));self.assertEqual(v.role_fds(17,"single"),(3,4,5,6,7,8,9))
        for index,actor in ((17,"a"),(0,"single"),(1,"b"),(True,"single"),(28,"single")):
            with self.subTest(index=index,actor=actor),self.assertRaises(ValueError):v.role_fds(index,actor)

    def test_closed_json_and_short_read_eof(self):
        for data in (b'{"a":1,"a":2}',b'{"a":NaN}',b'{} {}',b'\xff',b''):
            with self.subTest(data=data),self.assertRaises((ValueError,UnicodeError)):v.closed_json(data)
        self.assertEqual(v.barrier_bytes([v.BARRIER[:1],v.BARRIER[1:7],v.BARRIER[7:]],True),v.BARRIER)
        for chunks,eof in (([v.BARRIER],False),([v.BARRIER[:-1]],True),([v.BARRIER+b"X"],True),([b"BAD\n"],True),([b""],True)):
            with self.subTest(chunks=chunks,eof=eof),self.assertRaises(ValueError):v.barrier_bytes(chunks,eof)

    def test_metadata_exact_dictionary_and_types(self):
        envelopes,_,_=batch_fixture();meta=envelopes[0]["actors"][0]["metadata"];v.validate_metadata(meta)
        mutations=(lambda m:m.update(executionReady=True),lambda m:m["fdRoles"].update({"9":"providedPrivateTun"}),
                   lambda m:m["fdObservedIdentities"]["8"].update(access="readWrite"),lambda m:m["budgets"].update(setupSeconds=True),
                   lambda m:m["caseOptions"].update(netnsFD=3.0),lambda m:m["caseOptions"].update(rulePriority=12000.0),
                   lambda m:m["caseOptions"].update(opaqueFactory="callback"),lambda m:m["topology"].update(innerHostFD=11))
        for mutation in mutations:
            changed=copy.deepcopy(meta);mutation(changed)
            with self.subTest(mutation=mutation),self.assertRaises(ValueError):v.validate_metadata(changed)

    def test_original_go_stdout_no_skip_extra_or_missing(self):
        envelopes,_,_=batch_fixture();data=base64.b64decode(envelopes[1]["actors"][0]["stdout"]);v.go_transcript(data,1,0)
        for changed,status in ((data.replace(b"--- PASS:",b"--- SKIP:",1),0),(data+b"=== RUN   TestExtra\n",0),(b"",0),(data,1)):
            with self.subTest(status=status),self.assertRaises(ValueError):v.go_transcript(changed,1,status)


class RawDumpTests(unittest.TestCase):
    def test_complete_independent_selector_bytes(self):
        value,raw=raw_fixture();self.assertEqual(v.netlink_dump(value),value["decoded"])
        # Nested attribute flags are retained with their entire raw payload, not dropped.
        nested=copy.deepcopy(value);changed=bytearray(raw);struct.pack_into("<H",changed,30,0x800f)
        nested["receives"][0]["rawDatagrams"]=base64.b64encode(changed).decode();nested["decoded"][0]["attributes"]={"32783":"409c0000"}
        self.assertEqual(v.netlink_dump(nested),nested["decoded"])

    def test_finite_raw_receive_negative_matrix(self):
        value,raw=raw_fixture()
        variants=[]
        for key,new in (("senderPortID",3),("senderGroups",1),("recvFlags",0x20),("receivedLength",1)):
            x=copy.deepcopy(value);x["receives"][0][key]=new;variants.append((key,x))
        for label,offset,fmt,new in (("seq",8,"<I",8),("local",12,"<I",10),("DUMP_INTR",6,"<H",0x12),("not_MULTI",6,"<H",0),("NLMSG_ERROR",4,"<H",2),("bad_attr_len",28,"<H",99),("DONE_error",len(raw)-4,"<i",-105),("ENOBUFS",4,"<H",4)):
            changed=bytearray(raw);struct.pack_into(fmt,changed,offset,new);x=copy.deepcopy(value);x["receives"][0]["rawDatagrams"]=base64.b64encode(changed).decode();variants.append((label,x))
        for label,changed in (("missing_DONE",raw[:-20]),("truncated",raw[:-1]),("after_DONE",raw+raw[-20:]),("missing_DONE_status",raw[:-4])):
            x=copy.deepcopy(value);x["receives"][0].update(rawDatagrams=base64.b64encode(changed).decode(),receivedLength=len(changed));variants.append((label,x))
        x=copy.deepcopy(value);x["decoded"][0]["attributes"]["15"]="00000000";variants.append(("spec_splice",x))
        for label,changed in variants:
            with self.subTest(label=label),self.assertRaises(ValueError):v.netlink_dump(changed)


class BatchTests(unittest.TestCase):
    def test_all28_logic_only_never_functional_pass(self):
        envelopes,plan,plan_bytes=batch_fixture()
        result=v.verify_batch(envelopes,plan,plan_bytes)
        self.assertEqual(result["aggregateResult"],"LogicOnly");self.assertEqual(result["consumerCoverage"],"NotReady")
        self.assertEqual(result["rCalleeCoverage"],"LogicOnly");self.assertIn("Unknown",result["subjectCleanup"]);self.assertIn("PartialSealed",result["subjectCleanup"])
        self.assertEqual(set(result),{"testAssertions","rCalleeCoverage","consumerCoverage","subjectCleanup","fixtureDisposal","guardianSettlement","hostUnchanged","aggregateResult"})

    def test_batch_finite_mutations_reject_splicing_and_laundering(self):
        original,plan,plan_bytes=batch_fixture()
        def change_event(batch,index,actor,key,value):
            batch[index]["actors"][actor]["evidence"][-1][key]=value;restream(batch[index]["actors"][actor],index)
        variants=(
            ("missing_case",lambda b:b.pop()),("duplicate_case",lambda b:b.__setitem__(27,copy.deepcopy(b[26]))),
            ("case_nonce",lambda b:b[1]["binding"].update(caseNonce=b[0]["binding"]["caseNonce"])),
            ("candidate",lambda b:b[1]["binding"].update(elfSha256="f"*64)),("config",lambda b:b[0]["actors"][0]["metadata"].update(configSha256="f"*64)),
            ("list",lambda b:b[0]["actors"][0]["list"].update(stdout="TestOther\n")),("FD10_B",lambda b:b[0]["actors"][1]["metadata"]["fdRoles"].update({"10":v.FD_ROLES[10]})),
            ("barrier_wait",lambda b:b[0]["barrier"].update(bWaitBeforeWrite=False)),("barrier_eof",lambda b:b[0]["barrier"].update(closedAfterWrite=False)),
            ("actor_birth",lambda b:change_event(b,1,0,"birth","e"*32)),("typed_stdout",lambda b:b[1]["actors"][0]["evidence"][-1].update(subjectCleanup="Unknown")),
            ("wait",lambda b:b[1]["actors"][0]["wait"].update(actual=False)),("host_commands",lambda b:b[1]["host"]["after"][0].update(command=["true"])),
            ("host_drift",lambda b:b[1]["host"]["after"][0].update(stdout='[{"ifname":"foreign"}]')),
            ("claims",lambda b:b[1]["host"].update(claimsAfter={"absent":False})),("guardian",lambda b:b[1]["settlement"].update(controlledNamespaceLifetimeEnded=False)),
            ("Unknown_launder",lambda b:change_event(b,14,0,"subjectCleanup","Closed")),("partial_launder",lambda b:change_event(b,20,0,"subjectCleanup","Closed")),
            ("fixture_restore",lambda b:b[16]["fixture"]["writerAfter"].update(value="2\n")),("provided_identity",lambda b:b[17]["fixture"]["providedOriginalIdentity"].update(ino=999)),
            ("old_name_delete",lambda b:b[15]["fixture"]["disposedObjects"][0].update(ifindex=0)),("extra_actor",lambda b:b[1]["actors"].append(copy.deepcopy(b[1]["actors"][0]))))
        for label,mutation in variants:
            changed=copy.deepcopy(original);mutation(changed)
            with self.subTest(label=label),self.assertRaises(ValueError):v.verify_batch(changed,plan,plan_bytes)

    def test_actual_label_without_actual_frozen_plan_cannot_claim_observed(self):
        envelopes,_,_=batch_fixture();envelopes[0]["evidenceClass"]="ActualNative"
        with self.assertRaisesRegex(ValueError,"producer plan"):v.verify_batch(envelopes)

    def test_pending_closure_raw_dump_and_combined_packet_budget(self):
        envelopes,plan,plan_bytes=batch_fixture()
        a=envelopes[0]["actors"][0];b=envelopes[0]["actors"][1]
        a["evidence"][0]["packets"]=5;b["evidence"][0]["packets"]=5;restream(a,0);restream(b,0)
        with self.assertRaisesRegex(ValueError,"combined actor"):v.verify_batch(envelopes,plan,plan_bytes)
        envelopes,plan,plan_bytes=batch_fixture();a=envelopes[1]["actors"][0];a["evidence"][0]["phase"]="running";restream(a,1)
        with self.assertRaisesRegex(ValueError,"closure"):v.verify_batch(envelopes,plan,plan_bytes)
        envelopes,plan,plan_bytes=batch_fixture();a=envelopes[1]["actors"][0];raw,_=raw_fixture();raw["receives"][0]["recvFlags"]=0x20;a["evidence"][0]["rawDumps"]=[raw];restream(a,1)
        with self.assertRaisesRegex(ValueError,"raw recv"):v.verify_batch(envelopes,plan,plan_bytes)

    def test_source_gate_in_actual_main_precedes_resource_calls(self):
        with patch.object(h.os,"environ",{"POLARIS_NO_KERNEL_RUN":"1","POLARIS_R_NATIVE_RUN":"1"}),patch.object(h.sys,"argv",["harness","--profile",v.PROFILE]),patch.object(h.os,"open",side_effect=AssertionError("unexpected FD")),patch.object(h,"secure_directory",side_effect=AssertionError("unexpected directory")),patch.object(h.os,"geteuid",side_effect=AssertionError("unexpected root check")):
            with self.assertRaisesRegex(RuntimeError,"NO_KERNEL_RUN"):h.main()
        with patch.object(h.os,"environ",{"POLARIS_R_NATIVE_RUN":"1"}),patch.object(h.sys,"argv",["harness","--profile",v.PROFILE]),patch.object(h.os,"open",side_effect=AssertionError("unexpected FD")):
            with self.assertRaisesRegex(RuntimeError,"not reviewed"):h.main()

    def test_wrong_case_fd_graph_rejects_before_dup_or_popen(self):
        with patch("fcntl.fcntl",side_effect=AssertionError("unexpected dup")),patch.object(h.subprocess,"Popen",side_effect=AssertionError("unexpected native")):
            for index,actor,slots in ((0,"b",(3,4,5,6,7,8,10)),(1,"single",(3,4,5,6,7,8,9)),(17,"single",(3,4,5,6,7,8))):
                with self.subTest(index=index),self.assertRaisesRegex(RuntimeError,"FD closure"):h.r_spawn("/native.test",[],{fd:fd for fd in slots},index,actor,{},[])


if __name__=="__main__":unittest.main()
