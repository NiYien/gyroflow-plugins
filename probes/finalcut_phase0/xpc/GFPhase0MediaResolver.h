#import <Foundation/Foundation.h>


NS_ASSUME_NONNULL_BEGIN

FOUNDATION_EXPORT NSDictionary<NSString *, id> * _Nullable GFPhase0ResolveFinderURLs(
    NSArray<NSURL *> *urls,
    NSError **error
);

FOUNDATION_EXPORT NSDictionary<NSString *, id> * _Nullable GFPhase0ResolveFCPXMLData(
    NSData *data,
    NSError **error
);

NS_ASSUME_NONNULL_END
