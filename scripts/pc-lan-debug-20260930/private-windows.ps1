#Requires -Version 5.0
# Source boundary sentinel. File privacy is implemented by lan.py's original Win32 handles.
# This script neither creates files nor certifies ACLs/HANDLE waits on an untested platform.
# Private readyHandoff requests and original pipe replies belong to the Python controller/receiver.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if ([Environment]::OSVersion.Platform -ne [PlatformID]::Win32NT) {
    [Console]::Out.WriteLine('{"schema":"polaris-pc-lan-windows-source-v1","status":"UnsupportedPlatform","claims":[],"globalNoOwner":false,"managedReady":false,"releaseReady":false,"networkExact":false,"outboundReady":false}')
    exit 2
}
[Console]::Out.WriteLine('{"schema":"polaris-pc-lan-windows-source-v1","status":"SourcePreparedRuntimeUnverified","privateFileBackend":"OriginalWin32HandleInLanPy","claims":[],"globalNoOwner":false,"managedReady":false,"releaseReady":false,"networkExact":false,"outboundReady":false}')
exit 2
