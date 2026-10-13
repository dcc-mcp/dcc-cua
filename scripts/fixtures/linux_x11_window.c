/* Owned native windows for the release CLI's private-Xvfb acceptance test.
 * The real window-owning process advertises only its own PID. Python remains
 * the orchestrator and a deliberately forbidden negative target.
 */
#include <X11/Xatom.h>
#include <X11/Xlib.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

static Window window(Display *display, int x, int y, unsigned long color) {
    Window result = XCreateSimpleWindow(display, DefaultRootWindow(display),
                                        x, y, 80, 60, 0, 0, color);
    unsigned long pid = (unsigned long)getpid();
    XChangeProperty(display, result, XInternAtom(display, "_NET_WM_PID", False),
                    XA_CARDINAL, 32, PropModeReplace, (unsigned char *)&pid, 1);
    XMapWindow(display, result);
    return result;
}

int main(void) {
    Display *display = XOpenDisplay(NULL);
    if (!display) {
        fputs("private Xvfb display required\n", stderr);
        return 1;
    }
    Window target = window(display, 20, 20, 0xff0000);
    Window sibling = window(display, 200, 100, 0x00ff00);
    Window cover = None;
    XSync(display, False);
    printf("{\"pid\":%ld,\"target\":%lu,\"sibling\":%lu}\n",
           (long)getpid(), target, sibling);
    fflush(stdout);

    char command[32];
    while (fgets(command, sizeof(command), stdin)) {
        if (target && strcmp(command, "blue\n") == 0) {
            XSetWindowBackground(display, target, 0x0000ff);
            XClearWindow(display, target);
        } else if (!cover && strcmp(command, "cover\n") == 0) {
            cover = window(display, 30, 30, 0xffffff);
        } else if (cover && strcmp(command, "uncover\n") == 0) {
            XDestroyWindow(display, cover);
            cover = None;
        } else if (target && strcmp(command, "hide\n") == 0) {
            XUnmapWindow(display, target);
        } else if (target && strcmp(command, "destroy\n") == 0) {
            XDestroyWindow(display, target);
            target = None;
        } else {
            fputs("invalid fixture command or state\n", stderr);
            XCloseDisplay(display);
            return 2;
        }
        XSync(display, False);
        command[strcspn(command, "\n")] = '\0';
        printf("{\"ok\":\"%s\"}\n", command);
        fflush(stdout);
    }
    /* Closing stdin ends the owned process and destroys all remaining XIDs. */
    XCloseDisplay(display);
    return 0;
}
