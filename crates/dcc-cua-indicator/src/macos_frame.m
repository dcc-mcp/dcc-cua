#import <AppKit/AppKit.h>
#import <ApplicationServices/ApplicationServices.h>
#import <CoreGraphics/CoreGraphics.h>
#include <poll.h>
#include <unistd.h>

static CGRect exterior(CGRect window, NSUInteger edge, CGFloat width) {
    switch (edge) {
    case 0: return CGRectMake(CGRectGetMinX(window)-width, window.origin.y, width, window.size.height);
    case 1: return CGRectMake(CGRectGetMaxX(window), window.origin.y, width, window.size.height);
    case 2: return CGRectMake(window.origin.x, CGRectGetMinY(window)-width, window.size.width, width);
    default: return CGRectMake(window.origin.x, CGRectGetMaxY(window), window.size.width, width);
    }
}

static BOOL fitsDisplay(CGRect rect) {
    CGDirectDisplayID displays[16];
    uint32_t count = 0;
    if (CGGetActiveDisplayList(16, displays, &count) != kCGErrorSuccess) return NO;
    for (uint32_t i=0; i<count; ++i)
        if (CGRectContainsRect(CGDisplayBounds(displays[i]), rect)) return YES;
    return NO;
}

// WindowServer lists on-screen windows front-to-back, including higher layers,
// transient dialogs and other applications. Only rectangular uncovered bands count.
typedef struct {
    int pid, layer, sharing, systemDock;
    unsigned int window;
    int centerHit;
    double bounds[4];
} FrameBlocker;

extern AXError _AXUIElementGetWindow(AXUIElementRef, CGWindowID *);

static NSInteger hitWindow(CGPoint point) {
    CGFloat desktopTop = CGRectGetMaxY(CGDisplayBounds(CGMainDisplayID()));
    NSInteger hit = [NSWindow windowNumberAtPoint:NSMakePoint(point.x, desktopTop-point.y)
                       belowWindowWithWindowNumber:0];
    if (hit != 0) return hit;
    // A non-AppKit CLI can receive no window number. Ask the system-wide
    // accessibility hit test for this exact point, never the target app alone.
    AXUIElementRef system = AXUIElementCreateSystemWide();
    if (!system) return 0;
    AXUIElementRef element = NULL;
    AXError result = AXUIElementCopyElementAtPosition(system, point.x, point.y, &element);
    CFRelease(system);
    if (result != kAXErrorSuccess || !element) return 0;
    CFTypeRef containing = NULL;
    CGWindowID number = 0;
    if (AXUIElementCopyAttributeValue(element, kAXWindowAttribute, &containing) == kAXErrorSuccess &&
        containing && CFGetTypeID(containing) == AXUIElementGetTypeID()) {
        result = _AXUIElementGetWindow((AXUIElementRef)containing, &number);
    } else {
        result = _AXUIElementGetWindow(element, &number);
    }
    if (containing) CFRelease(containing);
    CFRelease(element);
    return result == kAXErrorSuccess ? number : 0;
}

int dcc_frame_point_hits(unsigned int window, double x, double y) {
    return hitWindow(CGPointMake(x,y)) == window;
}

static BOOL fullDisplayBounds(CGRect rect) {
    CGDirectDisplayID displays[16];
    uint32_t count = 0;
    if (CGGetActiveDisplayList(16, displays, &count) != kCGErrorSuccess) return NO;
    for (uint32_t i=0; i<count; ++i)
        if (CGRectEqualToRect(CGDisplayBounds(displays[i]), rect)) return YES;
    return NO;
}

int dcc_frame_exposed(int owner, unsigned int window, const int *rect, int renderer, int width, FrameBlocker *blocker) {
    @autoreleasepool {
        if (NSWorkspace.sharedWorkspace.frontmostApplication.processIdentifier != owner) return -1;
        CGRect target = CGRectMake(rect[0], rect[1], rect[2], rect[3]);
        if (!fitsDisplay(target)) return -2;
        NSArray *stack = CFBridgingRelease(CGWindowListCopyWindowInfo(
            kCGWindowListOptionOnScreenOnly, kCGNullWindowID));
        BOOL covered[4] = {NO, NO, NO, NO};
        BOOL visibleBand = NO;
        for (NSDictionary *entry in stack) {
            if ([entry[(__bridge id)kCGWindowAlpha] doubleValue] == 0) continue;
            CGRect bounds;
            if (!CGRectMakeWithDictionaryRepresentation((__bridge CFDictionaryRef)entry[(__bridge id)kCGWindowBounds], &bounds))
                return 0;
            int pid = [entry[(__bridge id)kCGWindowOwnerPID] intValue];
            if (pid == owner && [entry[(__bridge id)kCGWindowNumber] unsignedIntValue] == window) {
                if (!CGRectEqualToRect(bounds, target)) return -3;
                return renderer == 0 || visibleBand ? 1 : -4;
            }
            if (CGRectIntersectsRect(bounds, target)) {
                int layer = [entry[(__bridge id)kCGWindowLayer] intValue];
                NSNumber *sharing = entry[(__bridge id)kCGWindowSharingState];
                BOOL dock = NO;
                NSInteger hit = -1;
                if (layer == CGWindowLevelForKey(kCGDockWindowLevelKey) && fullDisplayBounds(bounds)) {
                    NSRunningApplication *app = [NSRunningApplication runningApplicationWithProcessIdentifier:pid];
                    dock = [app.bundleIdentifier isEqualToString:@"com.apple.dock"] &&
                        [app.executableURL.path isEqualToString:@"/System/Library/CoreServices/Dock.app/Contents/MacOS/Dock"];
                    hit = hitWindow(CGPointMake(CGRectGetMidX(target), CGRectGetMidY(target)));
                    // Tahoe's untitled Dock tracking surface spans the
                    // display even when WindowServer routes input through it.
                    // Real Dock panels and every non-system owner remain blockers.
                    if (dock && sharing && (sharing.intValue == kCGWindowSharingNone ||
                        sharing.intValue == kCGWindowSharingReadOnly) &&
                        [entry[(__bridge id)kCGWindowName] length] == 0 && hit == window) continue;
                }
                if (blocker) {
                    *blocker = (FrameBlocker){pid, layer, sharing ? sharing.intValue : -1, dock,
                        [entry[(__bridge id)kCGWindowNumber] unsignedIntValue], (int)hit,
                        {bounds.origin.x, bounds.origin.y, bounds.size.width, bounds.size.height}};
                }
                return -5;
            }
            for (NSUInteger edge=0; edge<4; ++edge) {
                CGRect band = exterior(target, edge, width);
                if (renderer > 0 && pid == renderer && !covered[edge] &&
                    [entry[(__bridge id)kCGWindowAlpha] doubleValue] == 1 &&
                    CGRectEqualToRect(bounds, band) && fitsDisplay(band))
                    visibleBand = YES;
                if (CGRectIntersectsRect(bounds, band)) covered[edge] = YES;
            }
        }
        return 0;
    }
}

static CGEventRef observeEscape(CGEventTapProxy proxy, CGEventType type, CGEventRef event, void *context) {
    (void)proxy;
    int *status = context;
    if (type == kCGEventTapDisabledByTimeout || type == kCGEventTapDisabledByUserInput) *status = 2;
    if (type == kCGEventKeyDown && CGEventGetIntegerValueField(event, kCGKeyboardEventKeycode) == 53)
        *status = 3;
    return event;
}

// Runs only on the main thread of a disposable renderer. EOF on the inherited
// control pipe releases the panels if the owning Host exits.
int dcc_run_frame(const int *rect, int width, int red, int green, int blue) {
    @autoreleasepool {
        [NSApplication sharedApplication];
        [NSApp setActivationPolicy:NSApplicationActivationPolicyAccessory];
        [NSApp finishLaunching];
        int status = 0;
        CFMachPortRef tap = CGEventTapCreate(kCGSessionEventTap, kCGHeadInsertEventTap,
            kCGEventTapOptionListenOnly, CGEventMaskBit(kCGEventKeyDown), observeEscape, &status);
        if (!tap) return 2;
        CFRunLoopSourceRef source = CFMachPortCreateRunLoopSource(NULL, tap, 0);
        if (!source) { CFRelease(tap); return 2; }
        CFRunLoopAddSource(CFRunLoopGetCurrent(), source, kCFRunLoopCommonModes);
        CGRect target = CGRectMake(rect[0], rect[1], rect[2], rect[3]);
        CGFloat desktopTop = CGRectGetMaxY(CGDisplayBounds(CGMainDisplayID()));
        NSMutableArray<NSPanel *> *bands = [NSMutableArray array];
        for (NSUInteger edge=0; edge<4; ++edge) {
            CGRect band = exterior(target, edge, width);
            if (!fitsDisplay(band)) continue;
            NSRect frame = NSMakeRect(band.origin.x, desktopTop-CGRectGetMaxY(band), band.size.width, band.size.height);
            NSPanel *panel = [[NSPanel alloc] initWithContentRect:frame
                styleMask:NSWindowStyleMaskBorderless|NSWindowStyleMaskNonactivatingPanel
                backing:NSBackingStoreBuffered defer:NO];
            panel.hasShadow = NO;
            panel.opaque = YES;
            panel.ignoresMouseEvents = YES;
            panel.hidesOnDeactivate = NO;
            panel.level = CGWindowLevelForKey(kCGDockWindowLevelKey) + 1;
            panel.backgroundColor = [NSColor colorWithSRGBRed:red/255.0 green:green/255.0 blue:blue/255.0 alpha:1];
            [panel orderFrontRegardless];
            [bands addObject:panel];
        }
        struct pollfd control = { .fd=STDIN_FILENO, .events=POLLIN };
        if (!bands.count || !CGEventTapIsEnabled(tap)) status = 2;
        while (!status && poll(&control, 1, 0) == 0) {
            @autoreleasepool { CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.008, false); }
            if (CGEventSourceKeyState(kCGEventSourceStateCombinedSessionState, 53)) status = 3;
        }
        for (NSPanel *band in bands) [band orderOut:nil];
        CFRunLoopRemoveSource(CFRunLoopGetCurrent(), source, kCFRunLoopCommonModes);
        CFMachPortInvalidate(tap);
        CFRelease(source);
        CFRelease(tap);
        return status;
    }
}
