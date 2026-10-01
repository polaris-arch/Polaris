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
    return h.r_original_options(index,actor)


def topology(index,observed=True):
    # Synthetic private observations are LogicOnly, never actual kernel evidence.
    persistent={"name":"rnt%02d"%index,"kind":"tun","linkIndex":77 if observed else None,"mtu":1500,"up":index==17,"addresses":["198.18.0.1/24"],"creatorQueueDisposition":"retained_on_FD9" if index==17 else "closed_before_R_attach"} if index in (15,16,17) else None
    return {"persistent":persistent,"foreignLink":{"name":"rnf0","kind":"dummy","linkIndex":77 if observed else None,"addresses":["198.18.253.1/24","fd00:729::1/64"]} if index==13 else None,
            "gateway":{"address":"198.18.0.2","subjectName":"rnt19","subjectPrefix":"198.18.0.1/24","routeWitness":"actual_subject_connected_route_after_New"} if index==19 else None,
            "ipv6Output":{"policy":"actual_min_positive_minus_one","beforePriorities":[0,32766,32767] if observed else None,"selectedPriority":32765 if observed else None} if index==25 else None,
            "providedTun":{"fd":9,"name":"rnt17","namespaceObservation":"getdevnetns_supported" if observed else None} if index==17 else None,
            "independentActors":{"aName":"rnt00","bName":"rnt00b","aTable":40000,"bTable":40100,"sharedPriority":12000,"controlBytes":"B_NEW_REFUSED\n","controlEOF":True} if index==0 else None,
            "rpFilter":{"all":0,"default":0,"deviceBefore":1 if index==16 else 0,"deviceStarted":2 if index==16 else 0,"deviceClosed":1 if index==16 else 0,"writablePath":"/proc/sys/net/ipv4/conf/rnt16/rp_filter" if index==16 else ""}}


def plan_fixture():
    manifest={"LogicOnly/synthetic.go":"2"*64};graph={"complete":True,"modules":[{"path":"LogicOnly","version":"synthetic","replace":None}]}
    plan={key:"3"*64 for key in ("harnessSha256","verifierSha256","profileSha256","configSha256","elfSha256","goModSha256","goSumSha256")}
    plan.update(schema=v.PLAN_SCHEMA,version=1,profile=v.PROFILE,sourceCommit="4"*40,sourceTree="5"*40,
                sourceManifest=manifest,sourceFilesSha256=hashlib.sha256(v.encoded(manifest)).hexdigest(),moduleGraph=graph,moduleGraphSha256=hashlib.sha256(v.encoded(graph)).hexdigest(),
                toolchain="LogicOnly-Go",buildFlags=["polaris_r_native"],batchNonce="6"*32,caseNonces=["%032x"%(index+1) for index in range(28)],
                cases=[],selectedTopLists=[],pcMetadataAck="0fdf9652adf4f69f222071c6062c5a7b263a619a38abe13c3d037f081a94db7b")
    for index,(top,sub) in enumerate(v.CASES):
        actors=["a","b"] if index==0 else ["single"]
        plan["cases"].append({"selected":top+"/"+sub,"actors":actors,"options":{actor:options(index,actor) for actor in actors},"topology":topology(index,False),
                              "expectedSubjectCleanup":{actor:"ConstructorClosedBeforeTUN" if actor=="b" else "UnknownOriginalCustody" if index==14 else "ReleasedOwnLedger" for actor in actors},
                              "expectedFixtureDisposal":"ActualRestoredAndDisposed"})
    plan["selectedTopLists"]=[{"top":top,"argv":["-test.list=^"+top+"$"],"stdout":top+"\n","exitStatus":0,"elfSha256":plan["elfSha256"]} for top,_ in v.GROUPS]
    return plan


def metadata(plan,index,actor,plan_bytes):
    binding={key:plan[key] for key in ("profileSha256","batchNonce","sourceCommit","sourceTree","sourceFilesSha256","moduleGraphSha256","elfSha256")}
    binding.update(caseNonce=plan["caseNonces"][index],planSha256=hashlib.sha256(plan_bytes).hexdigest(),configSha256=h.r_config_digest(plan["cases"][index]["options"][actor]))
    return {"schema":v.METADATA_SCHEMA,"version":1,"profile":v.PROFILE,**binding,"caseID":index,"actor":actor,
            "fdRoles":[{"fd":fd,"role":v.FD_ROLES[fd]} for fd in v.role_fds(index,actor)],
            "fdObservedIdentities":{v.FD_ROLES[fd]:{"device":1,"inode":100+fd} for fd in v.role_fds(index,actor)},
            "guardianParentIdentities":{name:{"device":1,"inode":i+1} for i,name in enumerate(("netns","mountns","userns","pidns","ipcns"))},
            "caseOptions":plan["cases"][index]["options"][actor],"topology":topology(index),"budgets":copy.deepcopy(v.BUDGETS)}


def event(meta,phase,value=None):
    kind="pending" if phase=="pending" else "terminal" if phase in ("actual_case_return","failed_case_return") else "phase"
    cleanup=("ConstructorClosedBeforeTUN" if meta["actor"]=="b" else "UnknownOriginalCustody" if meta["caseID"]==14 else "ReleasedOwnLedger") if phase=="actual_case_return" else "NotObserved"
    return {"schema":"polaris-r-native-case-evidence","version":1,"kind":kind,"phase":phase,
            **{key:meta[key] for key in ("batchNonce","caseNonce","caseID","actor")},"caseName":"/".join(v.CASES[meta["caseID"]]),
            "candidate":{key:meta[key] for key in ("profileSha256","sourceCommit","sourceTree","sourceFilesSha256","moduleGraphSha256","elfSha256","configSha256","planSha256")},
            "sequence":0,"encodedBytes":0,"packetCount":0,"subjectCleanup":cleanup,"testAssertions":"PASS" if phase=="actual_case_return" else "Pending",
            "rCalleeCoverage":"actual_R_API_only","consumerCoverage":"NotReady" if meta["caseID"]==0 else "NotInScope","fixtureDisposal":"GuardianPending","guardianSettlement":"GuardianPending","hostUnchanged":"GuardianPending","aggregateResult":"NotReady","value":value}


def actor_fixture(plan,index,actor,plan_bytes):
    meta=metadata(plan,index,actor,plan_bytes);birth="%032x"%(1000+2*index+(actor=="b"))
    events=[event(meta,"pending")]
    for phase in v.required_phases(index,actor):
        facts=None
        if phase in ("actual_start","independent_a_running_before_b"):facts={"Birth":birth,"Claims":[],"Observation":{"LogicOnly":True}}
        elif phase=="independent_b_allocator_priority_refusal":facts={"ActualBirth":birth,"Errno":"file exists","ReturnedNil":True,"TUNCalls":0,"Started":False,"Callback":False}
        elif phase=="actual_new_without_start_close":facts=birth
        elif phase=="actual_rp_filter_start":facts=2
        elif phase=="actual_rp_filter_close":facts=1
        events.append(event(meta,phase,facts))
    events.append(event(meta,"actual_case_return"))
    result={"actor":actor,"metadata":meta,"pid":2000+2*index+(actor=="b"),"wait":{"pid":2000+2*index+(actor=="b"),"exitStatus":0,"actual":True,"timedOut":False},
            "stderr":"","list":next(x for x in plan["selectedTopLists"] if x["top"]==v.CASES[index][0]),"evidence":events}
    restream(result,index)
    return result


def restream(actor,index):
    top,sub=v.CASES[index];total=0
    data=("=== RUN   "+top+"\n=== RUN   "+top+"/"+sub+"\n").encode()
    for seq,e in enumerate(actor["evidence"],1):
        e["sequence"]=seq
        for _ in range(6):
            line=b"R_NATIVE_EVIDENCE "+v.encoded(e,v.MAX_EVIDENCE)+b"\n"
            if e["encodedBytes"]==total+len(line):break
            e["encodedBytes"]=total+len(line)
        total+=len(line);data+=line
    data+=("--- PASS: "+top+"/"+sub+" (0.01s)\n--- PASS: "+top+" (0.01s)\nPASS\n").encode()
    actor["stdout"]=base64.b64encode(data).decode()


def batch_fixture():
    plan=plan_fixture();plan_bytes=v.encoded(plan,4194304);envelopes=[]
    for index,(top,sub) in enumerate(v.CASES):
        actors=[actor_fixture(plan,index,actor,plan_bytes) for actor in plan["cases"][index]["actors"]]
        binding={key:actors[0]["metadata"][key] for key in ("profileSha256","batchNonce","caseNonce","sourceCommit","sourceTree","sourceFilesSha256","moduleGraphSha256","elfSha256","configSha256","planSha256")}
        snapshots=[{"command":command,"status":0,"stdout":"[]" if command[0].endswith("ip") else "{}" if command[0].endswith("nft") else "LogicOnly-empty", "startedMonotonic":100,"finishedMonotonic":101,"stderr":""} for command in v.HOST_COMMANDS]
        persistent={"ifindex":77,"name":"rnt%02d"%index,"mtu":1500,"up":index==17,"addresses":[{"local":"198.18.0.1","prefixlen":24}]} if index in (15,16,17) else None
        fixture={"restored":True,"fixtureDisposal":"ActualRestoredAndDisposed","persistentBefore":persistent,"persistentAfter":copy.deepcopy(persistent),
                 "writerBefore":{"path":"/proc/sys/net/ipv4/conf/rnt16/rp_filter","identity":88,"value":"1\n"} if index==16 else None,
                 "writerAfter":{"identity":88,"value":"1\n"} if index==16 else None,"providedOriginalIdentity":{"dev":1,"ino":109} if index==17 else None,
                 "disposedObjects":[{"name":"rnt%02d"%index,"ifindex":77}] if index in (15,16,17) else [{"name":"rnf0","ifindex":77}] if index==13 else [],
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
    value={"request":26,"sequence":7,"localPortID":9,"senderPortID":0,"senderGroups":0,"recvFlags":0,"receivedLength":len(raw),"rawDatagrams":[base64.b64encode(raw).decode()],"receives":[{"senderPortID":0,"senderGroups":0,"recvFlags":0,"receivedLength":len(raw)}],"terminalStatus":"DONE_ZERO",
           "decoded":[{"type":24,"flags":2,"sequence":7,"portID":9,"fixedHeader":base64.b64encode(body[:12]).decode(),"attributes":[{"rawType":15,"type":15,"flags":0,"payload":base64.b64encode(body[16:]).decode()}]}, {"type":3,"flags":2,"sequence":7,"portID":9,"fixedHeader":"AAAAAA==","attributes":[]}]}
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
        mutations=(lambda m:m.update(executionReady=True),lambda m:m["fdRoles"].append({"fd":9,"role":"providedPrivateTun"}),
                   lambda m:m["fdObservedIdentities"]["metadata"].update(access="readWrite"),lambda m:m["budgets"].update(caseMilliseconds=True),
                   lambda m:m["caseOptions"]["constructor"].update(FileDescriptor=3.0),lambda m:m["caseOptions"]["constructor"].update(IPRoute2RuleIndex=12000.0),
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
        nested["rawDatagrams"][0]=base64.b64encode(changed).decode();nested["decoded"][0]["attributes"]=[{"rawType":32783,"type":15,"flags":32768,"payload":"QJwAAA=="}]
        self.assertEqual(v.netlink_dump(nested),nested["decoded"])

    def test_finite_raw_receive_negative_matrix(self):
        value,raw=raw_fixture()
        variants=[]
        wrong=copy.deepcopy(value);wrong["decoded"][-1]["flags"]=2.0;variants.append(("decoded_type",wrong))
        wrong=copy.deepcopy(value);wrong["senderGroups"]=False;variants.append(("aggregate_type",wrong))
        for key,new in (("senderPortID",3),("senderGroups",1),("recvFlags",0x20),("receivedLength",1)):
            x=copy.deepcopy(value);x["receives"][0][key]=new;variants.append((key,x))
        for label,offset,fmt,new in (("seq",8,"<I",8),("local",12,"<I",10),("DUMP_INTR",6,"<H",0x12),("not_MULTI",6,"<H",0),("NLMSG_ERROR",4,"<H",2),("bad_attr_len",28,"<H",99),("DONE_error",len(raw)-4,"<i",-105),("ENOBUFS",4,"<H",4)):
            changed=bytearray(raw);struct.pack_into(fmt,changed,offset,new);x=copy.deepcopy(value);x["rawDatagrams"][0]=base64.b64encode(changed).decode();variants.append((label,x))
        for label,changed in (("missing_DONE",raw[:-20]),("truncated",raw[:-1]),("after_DONE",raw+raw[-20:]),("missing_DONE_status",raw[:-4])):
            x=copy.deepcopy(value);x["rawDatagrams"][0]=base64.b64encode(changed).decode();x["receivedLength"]=len(changed);x["receives"][0].update(receivedLength=len(changed));variants.append((label,x))
        x=copy.deepcopy(value);x["decoded"][0]["attributes"][0]["payload"]="AAAAAA==";variants.append(("spec_splice",x))
        for label,changed in variants:
            with self.subTest(label=label),self.assertRaises(ValueError):v.netlink_dump(changed)


class BatchTests(unittest.TestCase):
    def test_all28_logic_only_never_functional_pass(self):
        envelopes,plan,plan_bytes=batch_fixture()
        result=v.verify_batch(envelopes,plan,plan_bytes)
        self.assertEqual(result["aggregateResult"],"LogicOnly");self.assertEqual(result["consumerCoverage"],"NotReady")
        self.assertEqual(result["rCalleeCoverage"],"LogicOnly");self.assertIn("UnknownOriginalCustody",result["subjectCleanup"]);self.assertIn("ReleasedOwnLedger",result["subjectCleanup"])
        self.assertEqual(set(result),{"testAssertions","rCalleeCoverage","consumerCoverage","subjectCleanup","fixtureDisposal","guardianSettlement","hostUnchanged","aggregateResult"})

    def test_batch_finite_mutations_reject_splicing_and_laundering(self):
        original,plan,plan_bytes=batch_fixture()
        def change_event(batch,index,actor,key,value):
            batch[index]["actors"][actor]["evidence"][-1][key]=value;restream(batch[index]["actors"][actor],index)
        variants=(
            ("missing_case",lambda b:b.pop()),("duplicate_case",lambda b:b.__setitem__(27,copy.deepcopy(b[26]))),
            ("case_nonce",lambda b:b[1]["binding"].update(caseNonce=b[0]["binding"]["caseNonce"])),
            ("candidate",lambda b:b[1]["binding"].update(elfSha256="f"*64)),("config",lambda b:b[0]["actors"][0]["metadata"].update(configSha256="f"*64)),
            ("list",lambda b:b[0]["actors"][0]["list"].update(stdout="TestOther\n")),("FD10_B",lambda b:b[0]["actors"][1]["metadata"]["fdRoles"].append({"fd":10,"role":v.FD_ROLES[10]})),
            ("barrier_wait",lambda b:b[0]["barrier"].update(bWaitBeforeWrite=False)),("barrier_eof",lambda b:b[0]["barrier"].update(closedAfterWrite=False)),
            ("actor_birth",lambda b:change_event(b,1,0,"value","e"*31)),("typed_stdout",lambda b:b[1]["actors"][0]["evidence"][-1].update(subjectCleanup="UnknownOriginalCustody")),
            ("wait",lambda b:b[1]["actors"][0]["wait"].update(actual=False)),("host_commands",lambda b:b[1]["host"]["after"][0].update(command=["true"])),
            ("host_drift",lambda b:b[1]["host"]["after"][0].update(stdout='[{"ifname":"foreign"}]')),
            ("claims",lambda b:b[1]["host"].update(claimsAfter={"absent":False})),("guardian",lambda b:b[1]["settlement"].update(controlledNamespaceLifetimeEnded=False)),
            ("Unknown_launder",lambda b:change_event(b,14,0,"subjectCleanup","Closed")),("partial_launder",lambda b:b[20]["actors"][0]["evidence"].pop(-3)),
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
        a["evidence"][-1]["packetCount"]=5;b["evidence"][-1]["packetCount"]=5;restream(a,0);restream(b,0)
        with self.assertRaisesRegex(ValueError,"packet count"):v.verify_batch(envelopes,plan,plan_bytes)
        envelopes,plan,plan_bytes=batch_fixture();a=envelopes[1]["actors"][0];a["evidence"][0]["phase"]="actual_start";restream(a,1)
        with self.assertRaises(ValueError):v.verify_batch(envelopes,plan,plan_bytes)
        envelopes,plan,plan_bytes=batch_fixture();a=envelopes[1]["actors"][0];raw,_=raw_fixture();raw["receives"][0]["recvFlags"]=0x20;a["evidence"][0]["value"]={"Raw":[raw]};restream(a,1)
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



class ActualPCDictionaryTests(unittest.TestCase):
    def test_all29_cross_language_frozen_original_digests_and_zero_defaults(self):
        # Literal hashes from the actual Go pure producer ace700a4, not computed
        # by this fixture or represented as actual native observations.
        expected = {(0, 'a'): '65be6a21ecaaf745d6c6da3dd98493a4c81f115ea258a26e584af0be87563590', (0, 'b'): '784976299a3a54d7f2d504e43cde78dbc04da984ab66ce2a0394fef9eb42f146', (1, 'single'): 'abf60d432da64743413a5d5055a8e4f60041a2af6e7fbdcac3d2e6dbbaffaca8', (2, 'single'): 'a591a4fbd009eafa52ddd7ba18fd6cdc64ca4aa023a91c3bc4b78d4bbdb07831', (3, 'single'): '7d16dec80efacb1a788059651ef8d84e047dd2f8f7c67bea7726e6d378060a3b', (4, 'single'): '5edfafc54ac89f7d852393f82c42df97c15a5144c85e3b39a768d9ecb6e0b141', (5, 'single'): 'ce8744a4594e08015e9091019d2e0cc091c52a45bc3fa174de364fe02f610270', (6, 'single'): 'e579d2fb8671ca615a8857bb064c1aca92991eaf242ce0b9048bbb2c8d0d2d11', (7, 'single'): '54a10dc4138e7646f21537c3231ff77e7c0530e4942a2eb92fb1d49bd6cbd6d2', (8, 'single'): '8aeef9f6a3d481fbba44c65b7977bf033e8286b69c5d292fbaf3fe96a69e2406', (9, 'single'): 'b50fe1f0269279fc8faa483bd5c21d08c6cf230972f9ee9eb2c29f2739897d05', (10, 'single'): '27334e39a68e45117314c2210ac2a602effe31871dcf9416d36151c83fb572ae', (11, 'single'): 'badb474738ca5353699e6a4629dc74d908e53e2f9f83e6afe01a237926a5626d', (12, 'single'): '918dc43a124c14a53234e0ef34a1701c49609f53fa4851ed292086cfea4d9a30', (13, 'single'): 'b82e61b31d30d8ca15b0c0f99671ccb3ace2ab3571d3c5d38f4ee6646aff221c', (14, 'single'): 'd7d63a10bdc29dfff1eef9779ab8d030fac2e3a6a9c438ce9bb2cc0d1d74295f', (15, 'single'): 'ee2aae9e4c64288995ab789865c743d46f00d5ed71c15a4d558dcae4c28c05c3', (16, 'single'): '97f7cf0ccdaba037ea4626973374ac61a0ae6123b14c565614e5c8532d269577', (17, 'single'): 'c29813414a196030ef817e443fe2a19fe72d6d6d8a36f7dd4e8989fa5c2fe9d9', (18, 'single'): '6933b4914223b9c0d1ced2bf78093c864c0e402a13423545c3dc6639b0b7c64e', (19, 'single'): '8500eed2dbaa200005dd7e3bfa5bc5c61e82f7c065aa1c806f11b627de601198', (20, 'single'): '1fd81cdb874e29779ec909aa08aac11d7422e08df5fad95d60eb609c47a3c3fb', (21, 'single'): '97169b7549b5e205b5bb223d7c6430db7f6622fe1d7fb862325cabfc4e2651d3', (22, 'single'): 'b0057e2fcfe467d5f910e87b2855aaa0969620b9bf821fb5442794932d0b4ba9', (23, 'single'): 'b1589ca215254b80307bbe37b34494e8a3ffa21bbfdf4d5e43c1dd7bbb982039', (24, 'single'): '48881f59af3d6e9738e78db2e56d664fa7dc50a3ab3d257b9db36afd4642e10d', (25, 'single'): 'dc750ec7882a3f101861655f4adbe0d797fc297f60a2d1127f1174083170419f', (26, 'single'): 'c95df674157adb95dfc79eb6bccf0c2dd4afd63eb57a454b6b3562a68360d2d6', (27, 'single'): 'd108ecfb7e37eeeda5e516a696c46aa44ff394c8e4edb1f8ab4189da436bb6ac'}
        self.assertEqual(len(expected),29)
        for (index,actor),digest in expected.items():
            body=options(index,actor)
            self.assertEqual(h.r_config_digest(body),digest)
            h.r_options(body,index,actor)
            for key in ("GSO","InterfaceScope","EXP_DisableDNSHijack"):
                bad=copy.deepcopy(body);bad["constructor"][key]=int(bad["constructor"][key])
                with self.subTest(index=index,actor=actor,key=key),self.assertRaises(RuntimeError):h.r_options(bad,index,actor)
            for key in ("Inet4RouteAddress","IncludeUID","Logger"):
                bad=copy.deepcopy(body);bad["constructor"][key]=[]
                with self.subTest(index=index,actor=actor,key=key),self.assertRaises(RuntimeError):h.r_options(bad,index,actor)
            bad=copy.deepcopy(body);bad["constructor"]["FileDescriptor"]=3
            with self.assertRaises(RuntimeError):h.r_options(bad,index,actor)

    def test_original_topology_metadata_and_runtime_observation_recipes(self):
        for index in range(28):
            h.r_topology(topology(index),index,observed=True)
            h.r_topology(topology(index,False),index)
            if index in (13,15,16,17,25):
                with self.assertRaises(RuntimeError):h.r_topology(topology(index,False),index,observed=True)
        bad=topology(0);bad["independentActors"]["controlEOF"]=1
        with self.assertRaises(RuntimeError):h.r_topology(bad,0,observed=True)
        bad=topology(13);bad["foreignLink"]["name"]="rfg13"
        with self.assertRaises(RuntimeError):h.r_topology(bad,13,observed=True)
        bad=topology(19);bad["gateway"]["routeWitness"]="prebuilt_subject_table"
        with self.assertRaises(RuntimeError):h.r_topology(bad,19,observed=True)
        bad=topology(25);bad["ipv6Output"]["selectedPriority"]+=1
        with self.assertRaises(RuntimeError):h.r_topology(bad,25,observed=True)

    def test_per_actor_config_and_pending_birth_cannot_be_whole_plan_or_future_birth(self):
        envelopes,plan,data=batch_fixture();a,b=envelopes[0]["actors"]
        self.assertNotEqual(a["metadata"]["configSha256"],b["metadata"]["configSha256"])
        self.assertNotEqual(a["metadata"]["configSha256"],plan["configSha256"])
        self.assertIsNone(v.actor_birth([a["evidence"][0]]))
        changed=copy.deepcopy(a["metadata"]);changed["configSha256"]=plan["configSha256"]
        with self.assertRaisesRegex(ValueError,"per-actor"):v.validate_metadata(changed)
        changed=copy.deepcopy(envelopes);changed[0]["actors"][0]["evidence"][0]["birth"]="f"*32;restream(changed[0]["actors"][0],0)
        with self.assertRaisesRegex(ValueError,"fields"):v.verify_batch(changed,plan,data)

    def test_actual_fd8_builder_all29_closed_inputs_under_pure_mocks(self):
        import os,stat,fcntl
        from types import SimpleNamespace
        plan=plan_fixture();planbytes=v.encoded(plan,4194304)
        for index in range(28):
            for actor in plan["cases"][index]["actors"]:
                held={3:103,4:104,5:105,6:106,7:107,8:108,9:109,10:110}
                data=bytearray()
                context={"plan":plan,"verifier":v,"index":index,"netfd":103,"userfd":104,"mntfd":105,"procFD":106,"claimsFD":107,"providedTun":109,"metadata":[],"planSha256":hashlib.sha256(planbytes).hexdigest(),"parent":{key:{"dev":1,"ino":i+1} for i,key in enumerate(("net","user","mnt","pid","ipc"))},"actualTopology":topology(index)}
                def fstat(fd):
                    slot=next((slot for slot,actual in held.items() if actual==fd),None)
                    mode=stat.S_IFREG if slot in (None,8) else stat.S_IFDIR if slot in (6,7) else stat.S_IFCHR if slot==9 else stat.S_IFIFO if slot==10 else stat.S_IFREG
                    return SimpleNamespace(st_mode=mode,st_dev=1,st_ino=fd,st_rdev=os.makedev(10,200),st_size=len(data))
                def flags(fd,op):return os.O_RDWR if fd==109 else os.O_RDONLY|os.O_NONBLOCK if fd==110 else os.O_RDONLY
                def ioctl(fd,request):return {103:h.CLONE_NEWNET,104:0x10000000,105:0x00020000}[fd]
                def write(fd,value):data.extend(value);return len(value)
                with patch.object(h.os,"open",side_effect=[900,108]),patch.object(h.os,"fstat",side_effect=fstat),patch.object(h.os,"write",side_effect=write),patch.object(h.os,"fsync"),patch.object(h.os,"close"),patch.object(fcntl,"fcntl",side_effect=flags),patch.object(fcntl,"ioctl",side_effect=ioctl):
                    mapping,meta=h.r_metadata(context,actor,110 if actor=="a" else None)
                self.assertEqual(tuple(sorted(mapping)),v.role_fds(index,actor))
                self.assertEqual(v.closed_json(bytes(data)),meta)
                self.assertEqual(meta["configSha256"],h.r_config_digest(options(index,actor)))
                self.assertEqual(meta["fdObservedIdentities"]["metadata"],{"device":1,"inode":108})
                self.assertEqual(len(context["metadata"]),1)
                self.assertFalse(h.R_SOURCE_EXECUTION_READY)


    def test_raw_packet_length_counters_subject_guardian_and_case_phase_replay(self):
        batch,plan,data=batch_fixture()
        actor=batch[27]["actors"][0];meta=actor["metadata"]
        packet=event(meta,"actual_packet",{"direction":"actual_TUN_Read","length":4,"raw":"dGVzdA=="})
        actor["evidence"].insert(-1,packet)
        packet["packetCount"]=1;actor["evidence"][-1]["packetCount"]=1;restream(actor,27)
        v.verify_batch(batch,plan,data)
        for mutation in (lambda b:b[27]["actors"][0]["evidence"][-2]["value"].update(length=5),lambda b:b[27]["actors"][0]["evidence"][-1].update(hostUnchanged="PASS"),lambda b:b[27]["actors"][0]["evidence"][-2].update(phase="actual_rp_filter_start")):
            bad=copy.deepcopy(batch);mutation(bad);restream(bad[27]["actors"][0],27)
            with self.assertRaises(ValueError):v.verify_batch(bad,plan,data)
        bad=copy.deepcopy(batch);bad[27]["actors"][0]["evidence"][-1]["encodedBytes"]+=1
        # Original stdout is unchanged, so an edited record cannot replace bytes.
        with self.assertRaises(ValueError):v.verify_batch(bad,plan,data)


if __name__=="__main__":unittest.main()
