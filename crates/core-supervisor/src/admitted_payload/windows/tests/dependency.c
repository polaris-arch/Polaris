#define WIN32_LEAN_AND_MEAN
#include <windows.h>
static void mark(const char *s, DWORD n) { static WCHAR p[32768]; DWORD z=GetEnvironmentVariableW(L"POLARIS_PRIVATE_IMAGE_MARKER",p,32768); if(!z || z>=32768) ExitProcess(251); HANDLE h=CreateFileW(p,FILE_APPEND_DATA,FILE_SHARE_READ|FILE_SHARE_WRITE,0,OPEN_ALWAYS,FILE_ATTRIBUTE_NORMAL,0); if(h==INVALID_HANDLE_VALUE) ExitProcess(252); DWORD w=0; if(!WriteFile(h,s,n,&w,0)||w!=n) ExitProcess(253); CloseHandle(h); }
__declspec(dllexport) DWORD fixture_value(void) { return VALUE; }
BOOL WINAPI DllMain(HINSTANCE h,DWORD reason,LPVOID reserved) { (void)h;(void)reserved; if(reason==DLL_PROCESS_ATTACH) {
#if VALUE == 65
 mark("DLL_A\n",6);
#else
 mark("DLL_B\n",6);
#endif
 } return TRUE; }
