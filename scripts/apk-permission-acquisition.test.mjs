import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, writeFileSync, mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { aapt2Path, apkPermissions, permissionViolations, APK_PERMISSION_REGISTRY, APK_DECLARED_PERMISSIONS } from './verify-apk.mjs';
const uri='http://schemas.android.com/apk/res/android', name=APK_DECLARED_PERMISSIONS[0].name;
// Source-version-bound format contract; not represented as captured AAPT2 output.
const xml=(level='0x00000002', extra='')=>`N: android=${uri} (line=1)
  E: manifest (line=1)
      E: permission (line=2)
        A: ${uri}:name(0x01010003)="${name}" (Raw: "${name}")
        A: ${uri}:protectionLevel(0x01010009)=${level}
${extra}      E: application (line=3)`;
const names=['package: com.polaris2.app',`permission: ${name}`,...APK_PERMISSION_REGISTRY.map(x=>`uses-permission: name='${x.name}'`)].join('\n');
function acquire(text) {
 const calls=[];
 const result=apkPermissions('fixture.apk',{bin:'aapt2-interface',run(bin,args){calls.push(args);return Buffer.from(args[1]==='permissions'?names:text);}});
 assert.deepEqual(calls,[['dump','permissions','fixture.apk'],['dump','xmltree','--file','AndroidManifest.xml','fixture.apk']]); return result;
}
test('NA05 actual acquisition interface: URI/PrettyPrint integer/Raw; exact signature',()=>{
 for(const value of ['0x00000002','2','0x00000002 (Raw: "signature")']) assert.deepEqual(permissionViolations(acquire(xml(value))),[]);
 for(const value of ['0','1','0x00000012','0x100000002','4294967298','-1','2.0','@0x01010002','true','"signature"','(type 0x11)0x2','2 junk']) assert.ok(permissionViolations(acquire(xml(value))).length>0,value);
 assert.ok(permissionViolations(acquire(xml().replace(/.*:protectionLevel.*\n/,''))).length>0);
 assert.ok(permissionViolations(acquire(xml('2',`        A: ${uri}:protectionLevel(0x01010009)=2\n`))).length>0);
 assert.ok(permissionViolations(acquire(xml().replace('(0x01010009)','(0x01010003)'))).length>0);
});
test('NA05 real SDK AAPT2 reads original compiled manifest and permission-only byte mutations',(t)=>{
 const bin=aapt2Path(); assert.ok(bin,'Real acquisition requires Android SDK AAPT2; no skipped pass');
 t.diagnostic(`AAPT2 version: ${execFileSync(bin,['version'],{encoding:'utf8'}).trim()}`);
 const dir=new URL('./fixtures/aapt2/',import.meta.url), meta=JSON.parse(readFileSync(new URL('provenance.json',dir)));
 const raw=Buffer.from(readFileSync(new URL('sfa-46eb-AndroidManifest.axml.base64',dir),'utf8').trim(),'base64');
 assert.equal(createHash('sha256').update(raw).digest('hex'),meta.manifestSha256);
 const declaration=meta.permissions[0], level=declaration.find(x=>x.name==='protectionLevel'), originalName=declaration.find(x=>x.name==='name').string;
 assert.equal(raw[level.dataOffset-1],0x11);assert.equal(raw.readUInt32LE(level.dataOffset),2);
 const root=mkdtempSync(join(tmpdir(),'aapt2-acquisition-'));
 try {for(const value of [2,0,1,0x12]){
   const bytes=Buffer.from(raw);bytes.writeUInt32LE(value,level.dataOffset);
   const manifest=join(root,'AndroidManifest.xml'), apk=join(root,'manifest-only.apk');writeFileSync(manifest,bytes);
   execFileSync(process.platform==='win32'?'python':'python3',['-c','import sys,zipfile; z=zipfile.ZipFile(sys.argv[2],"w"); z.write(sys.argv[1],"AndroidManifest.xml"); z.close()',manifest,apk]);
   const result=apkPermissions(apk,{bin});assert.ok(result);assert.equal(result.packageName,meta.packageName);
   assert.equal(result.declaredProtectionLevels[originalName],value);assert.deepEqual(result.declared,[originalName]);
   assert.ok(permissionViolations(result).some(x=>x.includes('applicationId'))); // SFA is not relabeled as App evidence.
 }}finally{rmSync(root,{recursive:true,force:true});}
});
