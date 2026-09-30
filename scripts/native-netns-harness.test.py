#!/usr/bin/env python3
"""Pure snapshot comparisons; no privilege or network operations."""
import importlib.util
import json
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location("native_harness", Path(__file__).with_name("native-netns-harness.py"))
harness = importlib.util.module_from_spec(spec)
spec.loader.exec_module(harness)


def pair(command, before, after, elapsed=10):
    return harness.compare_snapshots(
        [{"command": command, "stdout": json.dumps(before), "startedMonotonic": 100}],
        [{"command": command, "stdout": json.dumps(after), "startedMonotonic": 100+elapsed}])


class SnapshotTests(unittest.TestCase):
    def test_native_missing_skip_and_receipt_are_rejected(self):
        expected = ["TestT2", "TestT3"]
        stdout = "=== RUN   TestT2\n--- PASS: TestT2 (0.01s)\n=== RUN   TestT3\n--- PASS: TestT3 (0.01s)\n"
        names = [name+".ndjson" for name in expected]
        self.assertTrue(harness.native_results(stdout, expected, 0, names)["complete"])
        self.assertFalse(harness.native_results(stdout.replace("PASS: TestT3", "SKIP: TestT3"), expected, 0, names)["complete"])
        self.assertFalse(harness.native_results("", expected, 0, names)["complete"])
        self.assertFalse(harness.native_results(stdout, expected, 0, names[:1])["complete"])
        self.assertFalse(harness.native_results(stdout, expected, 1, names)["complete"])

    def test_only_monotonic_known_address_clock_is_normalized(self):
        command = ["/usr/sbin/ip", "-j", "address", "show"]
        before = [{"ifname": "lo", "addr_info": [{"local": "192.0.2.1", "prefixlen": 24, "valid_life_time": 100}]}]
        after = [{"ifname": "lo", "addr_info": [{"local": "192.0.2.1", "prefixlen": 24, "valid_life_time": 90}]}]
        result = pair(command, before, after)
        self.assertTrue(result["equal"])
        self.assertEqual(result["normalizations"][0]["path"], "$[0].addr_info[0].valid_life_time")
        after[0]["addr_info"][0]["valid_life_time"] = 110
        self.assertFalse(pair(command, before, after)["equal"])
        after[0]["addr_info"][0]["valid_life_time"] = 1
        self.assertFalse(pair(command, before, after)["equal"])

    def test_unrelated_expiry_and_route_identity_are_retained(self):
        command = ["/usr/sbin/ip", "-j", "route", "show"]
        self.assertFalse(pair(command, [{"dst": "192.0.2.0/24", "timeout": 100}], [{"dst": "192.0.2.0/24", "timeout": 90}])["equal"])
        self.assertFalse(pair(command, [{"dst": "192.0.2.0/24", "expires": 100}], [{"dst": "198.51.100.0/24", "expires": 90}])["equal"])

    def test_nft_counter_identity_and_timeout_are_retained(self):
        command = ["/usr/sbin/nft", "--json", "list", "ruleset"]
        before = {"nftables": [{"counter": {"name": "a", "table": "t", "packets": 1, "bytes": 8}}]}
        after = {"nftables": [{"counter": {"name": "a", "table": "t", "packets": 2, "bytes": 16}}]}
        self.assertTrue(pair(command, before, after)["equal"])
        after["nftables"][0]["counter"]["name"] = "b"
        self.assertFalse(pair(command, before, after)["equal"])
        after["nftables"][0]["counter"]["name"] = "a"
        after["nftables"][0]["counter"]["packets"] = 0
        self.assertFalse(pair(command, before, after)["equal"])
        self.assertFalse(pair(command, {"nftables": [{"set": {"timeout": 100}}]}, {"nftables": [{"set": {"timeout": 90}}]})["equal"])


class RSourceTests(unittest.TestCase):
    def test_r_deny_precedes_every_effect_and_metadata_flag_cannot_issue_admission(self):
        with self.assertRaisesRegex(RuntimeError, "NO_KERNEL_RUN"):
            harness.r_execution_gate({"POLARIS_NO_KERNEL_RUN":"0","POLARIS_R_NATIVE_RUN":"1"})
        with self.assertRaisesRegex(RuntimeError, "opt-in"):
            harness.r_execution_gate({})
        with self.assertRaisesRegex(RuntimeError, "not reviewed"):
            harness.r_execution_gate({"POLARIS_R_NATIVE_RUN":"1"})
        self.assertFalse(harness.R_SOURCE_EXECUTION_READY)
        self.assertFalse(harness.R_METADATA_ACK)
        with self.assertRaisesRegex(RuntimeError, "washed out"):
            harness.r_child_environment({"POLARIS_NO_KERNEL_RUN":"1"}, {"executionReady":True})

    def test_only_two_actual_r_socket_syscalls_and_six_frozen_ioctls_are_admitted(self):
        import struct
        def execute(program, arch, number, args):
            seccomp_data=struct.pack("<iIQ6Q",number,arch,0,*args,*([0]*(6-len(args))))
            accumulator=0;position=0
            while position<len(program):
                code,jt,jf,value=program[position]
                if code==0x20:accumulator=struct.unpack_from("<I",seccomp_data,value)[0]
                elif code==0x54:accumulator &= value
                elif code==0x15:position+=jt if accumulator==value else jf
                elif code==0x05:position+=value
                elif code==0x06:return value
                else:self.fail("unexpected effect-filter instruction")
                position+=1
            self.fail("unterminated BPF")
        for architecture,arch,socket,ioctl in (("x86_64",0xc000003e,41,16),("aarch64",0xc00000b7,198,29)):
            program=harness.r_effect_program(architecture)
            self.assertEqual(execute(program,arch,999,[]),0x7fff0000)
            self.assertEqual(execute(program,0,999,[]),0x80000000)
            if architecture=="x86_64":
                for number in (socket,ioctl,999):
                    self.assertEqual(execute(program,arch,number|0x40000000,[]),0x00050001)
            for family,kind,protocol in harness.R_SOCKET_TRIPLES:
                for flags in (0,0x800,0x80000,0x80800):
                    self.assertEqual(execute(program,arch,socket,[family,kind|flags,protocol]),0x7fff0000)
                for which in range(3):
                    values=[family,kind,protocol];values[which]|=1<<32
                    self.assertEqual(execute(program,arch,socket,values),0x00050001)
            for values in ([2,1,6],[17,3,0],[16,3,12],[1,1,0],[10,2,6],[2,2|0x4000,17]):
                self.assertEqual(execute(program,arch,socket,values),0x00050001)
            for request in harness.R_IOCTLS:
                self.assertEqual(execute(program,arch,ioctl,[3,request,0]),0x7fff0000)
                self.assertEqual(execute(program,arch,ioctl,[3,request|(1<<32),0]),0x00050001)
            for request in (0x400454cb,0xb702,0x400454d9,0x400454d0,0x5413):
                self.assertEqual(execute(program,arch,ioctl,[3,request,0]),0x00050001)

    def test_nine_original_host_snapshots_and_n_default_are_retained(self):
        from unittest.mock import patch
        seen=[]
        with patch.object(harness,"command",side_effect=lambda args,timeout=5:seen.append((args,timeout)) or {"command":args}):
            self.assertEqual(len(harness.snapshot()),9)
        self.assertTrue(all(timeout==5 for _,timeout in seen))
        self.assertEqual(seen[0][0],["/usr/sbin/ip","-j","link","show"])
        self.assertEqual(seen[6][0],["/usr/sbin/nft","--json","list","ruleset"])
        self.assertEqual(harness.N_PROFILE,"nft-only-child-userns-v1")

    def test_r_layer_contains_no_generic_native_command_or_provided_host_fd(self):
        import ast
        source=Path(harness.__file__).read_text();tree=ast.parse(source)
        functions={n.name:n for n in tree.body if isinstance(n,ast.FunctionDef)}
        main=ast.get_source_segment(source,functions["main"])
        self.assertLess(main.index("r_execution_gate(os.environ)"),main.index("secure_directory("))
        worker=ast.get_source_segment(source,functions["worker"])
        self.assertLess(worker.index("r_execution_gate(os.environ)"),worker.index("os.open("))
        spawn=ast.get_source_segment(source,functions["r_spawn"])
        self.assertIn("fcntl.F_DUPFD_CLOEXEC,32",spawn)
        self.assertLess(spawn.index("children.append(child)"),spawn.index("for slot,source in saved.items()"))
        self.assertIn("pass_fds=tuple(sorted(mapping)),close_fds=True",spawn)
        self.assertIn("r_native_landlock(index);r_install_effect_filter()",spawn)
        loop=ast.get_source_segment(source,functions["r_children"])
        self.assertNotIn("-test.list",loop)
        self.assertIn('r_spawn("/native.test"',loop)
        self.assertIn("r_control_write",loop)
        self.assertNotIn("for index in range(28)",loop)
        self.assertIn('"bWaitBeforeWrite":True',loop)
        native_fence=ast.get_source_segment(source,functions["r_native_landlock"])
        self.assertIn('if index==16:',native_fence)
        self.assertNotIn('/conf/all/',native_fence)
        self.assertNotIn('/conf/default/',native_fence)


class RMetadataLandlockP1Tests(unittest.TestCase):
    def paths(self, source, function, context):
        # Evaluate only the actual finite paths assignments, never a fence/syscall/fixture.
        import ast
        tree=ast.parse(source);node=next(n for n in tree.body if isinstance(n,ast.FunctionDef) and n.name==function)
        environment={"writable":(1<<1)|sum(1<<bit for bit in range(4,15)),**context}
        values=[]
        def walk(statements):
            nonlocal values
            for statement in statements:
                if isinstance(statement,ast.Assign) and any(isinstance(t,ast.Name) and t.id=="paths" for t in statement.targets):
                    values=eval(compile(ast.Expression(statement.value),"LogicOnly paths","eval"),{"__builtins__":{}},environment)
                elif isinstance(statement,ast.AugAssign) and isinstance(statement.target,ast.Name) and statement.target.id=="paths":
                    values+=eval(compile(ast.Expression(statement.value),"LogicOnly paths","eval"),{"__builtins__":{}},environment)
                elif isinstance(statement,ast.If):
                    admitted=eval(compile(ast.Expression(statement.test),"LogicOnly paths condition","eval"),{"__builtins__":{}},environment)
                    walk(statement.body if admitted else statement.orelse)
                elif isinstance(statement,ast.Try):walk(statement.body)
                elif isinstance(statement,ast.For):
                    # All remaining loops are actual operations; never evaluate them.
                    continue
        # Before the original Try all statements except paths are actual operations.
        if function=="filesystem_and_syscall_fence":walk(next(x for x in node.body if isinstance(x,ast.Try)).body)
        else:walk([x for x in node.body if isinstance(x,(ast.Assign,ast.AugAssign,ast.If)) and (not isinstance(x,ast.Assign) or any(isinstance(t,ast.Name) and t.id=="paths" for t in x.targets))])
        return dict(values)

    def test_actual_launcher_metadata_make_and_write_and_missing_rule_mutant(self):
        source=Path(harness.__file__).read_text()
        context={"r_context":{"index":16,"sysctls":{"/proc/sys/net/ipv4/conf/all/rp_filter":{},"/proc/sys/net/ipv4/conf/default/rp_filter":{}}}}
        def require_exact(value):
            rules=self.paths(value,"filesystem_and_syscall_fence",context)
            self.assertEqual(rules.get("/r-metadata"),(1<<1)|(1<<8))
        require_exact(source)
        missing=source.replace('paths += [("/r-metadata", (1 << 1) | (1 << 8))]','pass')
        self.assertNotEqual(source,missing)
        with self.assertRaises(AssertionError):require_exact(missing)
        self.assertNotIn("/r-metadata",self.paths(source,"filesystem_and_syscall_fence",{"r_context":None}))

    def test_actual_native_child_still_excludes_metadata_write_and_widening_mutant(self):
        source=Path(harness.__file__).read_text()
        def require_closed(value):
            for index in (0,16,17):
                self.assertNotIn("/r-metadata",self.paths(value,"r_native_landlock",{"index":index}))
        require_closed(source)
        wide=source.replace('if index==16:paths.append(("/proc/sys/net/ipv4/conf/rnt16/rp_filter",1<<1))','paths += [("/r-metadata", writable)]\n    if index==16:paths.append(("/proc/sys/net/ipv4/conf/rnt16/rp_filter",1<<1))')
        self.assertNotEqual(source,wide)
        with self.assertRaises(AssertionError):require_closed(wide)


# Classes appended above must be loaded before unittest discovers the full source suite.
if __name__ == "__main__":
    unittest.main()
