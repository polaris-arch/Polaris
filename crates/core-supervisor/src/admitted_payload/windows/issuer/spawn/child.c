#include <windows.h>
void mainCRTStartup(void) {
    DWORD n = 0;
    char input = 0;
    if (!WriteFile(GetStdHandle(STD_OUTPUT_HANDLE), "ready", 5, &n, 0) || n != 5) ExitProcess(7);
    if (!ReadFile(GetStdHandle(STD_INPUT_HANDLE), &input, 1, &n, 0) || n != 1) ExitProcess(8);
    ExitProcess(input == 'x' ? 0 : 9);
}
