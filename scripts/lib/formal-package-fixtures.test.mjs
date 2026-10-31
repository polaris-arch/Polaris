// Inert source controls. Linux/ZIP containers are real; 7-Zip-only formats use
// an explicit controlled backend, never native package/signature evidence.
import { cpSync, mkdirSync, copyFileSync, writeFileSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { execFileSync } from 'node:child_process';
export function copyPackagingSource(original, root) {
  mkdirSync(join(root, 'scripts'), { recursive: true });
  for (const dir of ['desktop-core', 'lib']) cpSync(join(original, 'scripts', dir), join(root, 'scripts', dir), { recursive: true, filter: p => !p.includes('__pycache__') && !p.includes('node_modules') });
  for (const name of ['verify-packaging.mjs','postprocess-appimage.mjs','release-assets.mjs','final-package-receipt.py','windows-package-integrity.py','native-payload-proof.py']) copyFileSync(join(original, 'scripts', name),join(root,'scripts',name));
  mkdirSync(join(root,'src-tauri'),{recursive:true});
  for (const name of ['core-manifest.json','build.rs','tauri.conf.json','tauri.linux.conf.json','tauri.windows.conf.json','tauri.macos-x64.conf.json','tauri.macos-arm64.conf.json']) copyFileSync(join(original,'src-tauri',name),join(root,'src-tauri',name));
}
export function writeFormalAssets(directory, names) {
  execFileSync('python3',['-B','-c',String.raw`
import json,sys,zipfile,subprocess,tempfile
from pathlib import Path
root=Path(sys.argv[1]);names=json.loads(sys.argv[2]);root.mkdir(parents=True,exist_ok=True)
for name in names:
 p=root/name
 if name.endswith(('.dmg','.exe')):
  files={'polaris.exe':'ordinary App','_up_/resources/win/polaris-helper.exe':'ordinary Helper'} if name.endswith('.exe') else {'Polaris.app/Contents/MacOS/polaris':'ordinary App','Polaris.app/Contents/Resources/_up_/resources/mac-x64/polaris-helper':'ordinary Helper'}
  p.write_text(json.dumps({'kind':'Nsis' if name.endswith('.exe') else 'HFS','files':files}))
 elif name.endswith(('.zip','.apk')):
  with zipfile.ZipFile(p,'w',compression=zipfile.ZIP_DEFLATED) as z:
   for n,data in ({'lib/arm64-v8a/libpolaris_lib.so':'ordinary App'} if name.endswith('.apk') else {'polaris.exe':'ordinary App','_up_/resources/win/polaris-helper.exe':'ordinary Helper'}).items():z.writestr(n,data)
 else:
  with tempfile.TemporaryDirectory() as tmp:
   stage=Path(tmp)
   for n,data in {'usr/bin/polaris':'ordinary App','usr/lib/Polaris/_up_/resources/linux/polaris-helper':'ordinary Helper'}.items():
    f=stage/n;f.parent.mkdir(parents=True,exist_ok=True);f.write_text(data)
   if name.endswith('.deb'):
    (stage/'DEBIAN').mkdir();(stage/'DEBIAN/control').write_text('Package: polaris-control\nVersion: 1.0\nArchitecture: amd64\nMaintainer: Polaris <test@example.invalid>\nDescription: inert source regression\n')
    subprocess.run(['dpkg-deb','--build','--root-owner-group',str(stage),str(p)],check=True,stdout=subprocess.DEVNULL)
   else:subprocess.run(['mksquashfs',str(stage),str(p),'-noappend','-processors','1','-quiet'],check=True,stdout=subprocess.DEVNULL)
`,directory,JSON.stringify(names)]);
  if (!names.some(n=>/\.(exe|dmg)$/.test(n))) return {...process.env};
  const tools=join(dirname(directory),'.fixture-tools');mkdirSync(tools,{recursive:true});
  writeFileSync(join(tools,'7zz'), '#!/usr/bin/env python3\nimport json,sys\nfrom pathlib import Path\na=sys.argv\nif a[1]=="i":print("7-Zip 24.09\\nNsis");sys.exit(0)\nd=json.loads(Path(a[-1] if a[1]=="l" else a[-2]).read_text())\nif a[1]=="l":\n print("Type = "+d["kind"]+"\\n----------\\n"+"\\n\\n".join("Path = "+n+"\\nSize = "+str(len(v.encode())) for n,v in d["files"].items()))\nelse:sys.stdout.buffer.write(d["files"][a[-1]].encode())\n',{mode:0o700});
  return { ...process.env, PATH: tools + ':' + process.env.PATH };
}
