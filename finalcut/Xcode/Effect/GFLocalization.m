#import "GFLocalization.h"

@interface GFLocalizationBundleMarker : NSObject
@end

@implementation GFLocalizationBundleMarker
@end

NSString *GFLocalized(NSString *key, NSString *fallback) {
    NSBundle *bundle = [NSBundle bundleForClass:[GFLocalizationBundleMarker class]];
    NSString *localized = [bundle localizedStringForKey:key value:fallback table:nil];
    return localized.length > 0 ? localized : fallback;
}
