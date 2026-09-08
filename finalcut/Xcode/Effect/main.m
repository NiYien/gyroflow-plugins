// SPDX-License-Identifier: GPL-3.0-or-later
#import <FxPlug/FxPlugSDK.h>
#import "GFLocalization.h"

@interface GFHostLanguageDelegate : NSObject <FxPrincipalDelegate>
@end

@implementation GFHostLanguageDelegate
- (void)didEstablishConnectionWithHost:(NSString *)identifier version:(NSString *)version {
    (void)version;
    GFConfigureLocalizationForHost(identifier);
}
@end

int main(int argc, const char *argv[]) {
    (void)argc;
    (void)argv;
    @autoreleasepool {
        GFHostLanguageDelegate *delegate = [[GFHostLanguageDelegate alloc] init];
        [FxPrincipal startServicePrincipalWithDelegate:delegate];
    }
    return 0;
}
