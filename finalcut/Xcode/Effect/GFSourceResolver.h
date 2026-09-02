#import <Foundation/Foundation.h>

NS_ASSUME_NONNULL_BEGIN

FOUNDATION_EXPORT NSDictionary<NSString *, id> * _Nullable GFResolveFinderURLs(
    NSArray<NSURL *> *urls,
    NSError **error
);

FOUNDATION_EXPORT NSDictionary<NSString *, id> * _Nullable GFResolveFCPXMLData(
    NSData *data,
    NSError **error
);

NS_ASSUME_NONNULL_END
