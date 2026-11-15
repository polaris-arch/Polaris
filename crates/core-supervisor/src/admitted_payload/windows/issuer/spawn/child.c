#include <windows.h>
void mainCRTStartup(void) {
    DWORD n = 0;
    char input = 0;
    if (!WriteFile(GetStdHandle(STD_OUTPUT_HANDLE), "ready", 5, &n, 0) || n != 5) ExitProcess(7);
    if (!ReadFile(GetStdHandle(STD_INPUT_HANDLE), &input, 1, &n, 0) || n != 1) ExitProcess(8);
    if (input == 'l') {
        static WCHAR path[32768];
        DWORD length = GetEnvironmentVariableW(L"POLARIS_CRONET_LIBRARY_PATH", path, 32768);
        if (!length || length >= 32768) ExitProcess(10);
        HMODULE library = LoadLibraryExW(path, 0, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32);
        if (!library) ExitProcess(GetLastError());
        if (!FreeLibrary(library)) ExitProcess(11);
        if (!WriteFile(GetStdHandle(STD_OUTPUT_HANDLE), "loaded", 6, &n, 0) || n != 6) ExitProcess(12);
        if (!ReadFile(GetStdHandle(STD_INPUT_HANDLE), &input, 1, &n, 0) || n != 1) ExitProcess(13);
    }
    ExitProcess(input == 'x' ? 0 : 9);
}
