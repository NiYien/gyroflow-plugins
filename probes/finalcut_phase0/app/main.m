#import <Cocoa/Cocoa.h>
#import <os/log.h>


int main(int argc, const char *argv[]) {
    (void)argc;
    (void)argv;
    @autoreleasepool {
        os_log_info(OS_LOG_DEFAULT, "phase0 wrapper launched bundle=%{public}@", NSBundle.mainBundle.bundleIdentifier);
    }
    return 0;
}
