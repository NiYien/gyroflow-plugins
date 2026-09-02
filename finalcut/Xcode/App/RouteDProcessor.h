#import <Foundation/Foundation.h>

NS_ASSUME_NONNULL_BEGIN

@interface RouteDProcessor : NSObject

+ (nullable NSDictionary<NSString *, NSData *> *)processFCPXML:(NSData *)input
                                                 processedName:(NSString *)processedName
                                                         error:(NSError **)error;
+ (nullable NSDictionary<NSString *, NSData *> *)processBatchFCPXML:(NSData *)input
                                                      processedName:(NSString *)processedName
                                                              error:(NSError **)error;

@end

NS_ASSUME_NONNULL_END
