#import "GFPhase0ProjectStore.h"


NSString *const GFPhase0ProjectStoreErrorDomain = @"com.niyien.gyroflow.finalcut.phase0-project";


@interface GFPhase0ProjectStore ()
@property(nonatomic, readwrite, nullable) NSData *currentProjectData;
@property(nonatomic, readwrite, nullable) NSNumber *currentProjectVersion;
@property(nonatomic, readwrite) NSString *status;
@end


@implementation GFPhase0ProjectStore

- (instancetype)init {
    self = [super init];
    if (self != nil) {
        self.status = @"Drop one media clip here";
    }
    return self;
}

- (BOOL)failWithCode:(GFPhase0ProjectStoreErrorCode)code
                  url:(NSURL *)url
              message:(NSString *)message
                error:(NSError **)error {
    NSString *preservation = self.currentProjectData != nil
        ? @"kept previous project"
        : @"no project loaded";
    self.status = [NSString stringWithFormat:@"Rejected %@: %@; %@",
                   url.lastPathComponent,
                   message,
                   preservation];
    if (error != NULL) {
        *error = [NSError errorWithDomain:GFPhase0ProjectStoreErrorDomain
                                     code:code
                                 userInfo:@{NSLocalizedDescriptionKey : message}];
    }
    return NO;
}

- (BOOL)importProjectURL:(NSURL *)url error:(NSError **)error {
    if (![[url.pathExtension lowercaseString] isEqualToString:@"gyroflow"]) {
        return [self failWithCode:GFPhase0ProjectStoreErrorWrongExtension
                              url:url
                          message:@"only .gyroflow project files are allowed"
                            error:error];
    }

    BOOL accessedSecurityScope = [url startAccessingSecurityScopedResource];
    NSError *readError = nil;
    NSData *data = [NSData dataWithContentsOfURL:url
                                        options:NSDataReadingMappedIfSafe
                                          error:&readError];
    if (accessedSecurityScope) {
        [url stopAccessingSecurityScopedResource];
    }
    if (data == nil) {
        NSString *message = [NSString stringWithFormat:@"project is not readable (%@)",
                             readError.localizedDescription ?: @"unknown error"];
        return [self failWithCode:GFPhase0ProjectStoreErrorReadFailed
                              url:url
                          message:message
                            error:error];
    }

    NSError *jsonError = nil;
    id object = [NSJSONSerialization JSONObjectWithData:data options:0 error:&jsonError];
    if (![object isKindOfClass:[NSDictionary class]]) {
        NSString *message = [NSString stringWithFormat:@"invalid JSON (%@)",
                             jsonError.localizedDescription ?: @"top-level value is not an object"];
        return [self failWithCode:GFPhase0ProjectStoreErrorInvalidJSON
                              url:url
                          message:message
                            error:error];
    }

    NSDictionary<NSString *, id> *project = (NSDictionary<NSString *, id> *)object;
    NSNumber *version = project[@"version"];
    NSDictionary *gyroSource = project[@"gyro_source"];
    if (![version isKindOfClass:[NSNumber class]] || version.integerValue < 3 ||
        ![gyroSource isKindOfClass:[NSDictionary class]]) {
        return [self failWithCode:GFPhase0ProjectStoreErrorInvalidStructure
                              url:url
                          message:@"project must contain version >= 3 and a gyro_source object"
                            error:error];
    }

    self.currentProjectData = [data copy];
    self.currentProjectVersion = version;
    self.status = [NSString stringWithFormat:@"Loaded %@ (%lu bytes)",
                   url.lastPathComponent,
                   (unsigned long)data.length];
    return YES;
}

- (void)recordAuthorizationCancellation {
    NSString *preservation = self.currentProjectData != nil
        ? @"kept previous project"
        : @"no project loaded";
    self.status = [NSString stringWithFormat:@"Authorization cancelled; %@", preservation];
}

@end
