#import <Foundation/Foundation.h>

NS_ASSUME_NONNULL_BEGIN

@interface RouteDProcessor : NSObject

+ (nullable NSDictionary<NSString *, NSData *> *)processBatchFCPXML:(NSData *)input
                                                        documentURL:(NSURL *)documentURL
                                                              error:(NSError **)error;

@end

NS_ASSUME_NONNULL_END
