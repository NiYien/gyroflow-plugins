#import "GFSourceResolver.h"

static NSString *const GFSourceResolverErrorDomain =
    @"com.niyien.gyroflow.finalcut.source-resolver";

static NSDictionary<NSString *, id> * _Nullable GFResolverFailure(
    NSString *message,
    NSError **error
) {
    if (error != NULL) {
        *error = [NSError errorWithDomain:GFSourceResolverErrorDomain
                                     code:1
                                 userInfo:@{NSLocalizedDescriptionKey : message}];
    }
    return nil;
}

static NSDictionary<NSString *, id> * _Nullable GFResolution(
    NSString *source,
    NSURL *mediaURL,
    NSError **error
) {
    if (!mediaURL.isFileURL || mediaURL.path.length == 0) {
        return GFResolverFailure(@"original media must be a local file URL", error);
    }
    NSURL *standardizedMediaURL = mediaURL.URLByStandardizingPath;
    NSURL *projectURL = [[standardizedMediaURL URLByDeletingPathExtension]
        URLByAppendingPathExtension:@"gyroflow"];
    BOOL isDirectory = NO;
    BOOL exists = [[NSFileManager defaultManager] fileExistsAtPath:projectURL.path
                                                       isDirectory:&isDirectory];
    return @{
        @"source" : source,
        @"mediaURL" : standardizedMediaURL.absoluteString,
        @"projectURL" : projectURL.absoluteString,
        @"projectExists" : @(exists && !isDirectory),
    };
}

NSDictionary<NSString *, id> * _Nullable GFResolveFinderURLs(
    NSArray<NSURL *> *urls,
    NSError **error
) {
    if (urls.count != 1) {
        return GFResolverFailure(@"drop must contain exactly one Finder file", error);
    }
    NSURL *mediaURL = urls.firstObject;
    if (!mediaURL.isFileURL) {
        return GFResolverFailure(@"Finder item must be a local file URL", error);
    }
    return GFResolution(@"finder", mediaURL, error);
}

NSDictionary<NSString *, id> * _Nullable GFResolveFCPXMLData(
    NSData *data,
    NSError **error
) {
    if (data.length == 0) {
        return GFResolverFailure(@"Final Cut pasteboard contains no FCPXML data", error);
    }
    NSError *parseError = nil;
    NSXMLDocument *document = [[NSXMLDocument alloc]
        initWithData:data
             options:NSXMLNodeLoadExternalEntitiesNever
               error:&parseError];
    if (document == nil) {
        return GFResolverFailure(
            [NSString stringWithFormat:@"invalid FCPXML: %@",
                                       parseError.localizedDescription ?: @"unknown parse error"],
            error
        );
    }
    NSArray<NSXMLNode *> *clipNodes = [document
        nodesForXPath:@"//*[local-name()='asset-clip' and @ref]"
                error:&parseError];
    if (clipNodes == nil) {
        return GFResolverFailure(
            [NSString stringWithFormat:@"unable to query FCPXML clips: %@",
                                       parseError.localizedDescription ?: @"unknown query error"],
            error
        );
    }
    if (clipNodes.count != 1) {
        return GFResolverFailure(@"FCPXML must contain exactly one clip reference", error);
    }
    NSArray<NSXMLNode *> *assetNodes = [document
        nodesForXPath:@"//*[local-name()='asset']"
                error:&parseError];
    if (assetNodes == nil) {
        return GFResolverFailure(
            [NSString stringWithFormat:@"unable to query FCPXML assets: %@",
                                       parseError.localizedDescription ?: @"unknown query error"],
            error
        );
    }
    if (assetNodes.count != 1) {
        return GFResolverFailure(@"FCPXML must contain exactly one asset", error);
    }

    NSXMLElement *clip = (NSXMLElement *)clipNodes.firstObject;
    NSXMLElement *asset = (NSXMLElement *)assetNodes.firstObject;
    NSString *clipReference = [clip attributeForName:@"ref"].stringValue;
    NSString *assetIdentifier = [asset attributeForName:@"id"].stringValue;
    if (clipReference.length == 0 || ![clipReference isEqualToString:assetIdentifier]) {
        return GFResolverFailure(@"the single clip must reference the single asset", error);
    }
    NSArray<NSXMLNode *> *originalRepresentations = [asset
        nodesForXPath:@"./*[local-name()='media-rep' and @kind='original-media' and @src]"
                error:&parseError];
    if (originalRepresentations == nil) {
        return GFResolverFailure(
            [NSString stringWithFormat:@"unable to query original media: %@",
                                       parseError.localizedDescription ?: @"unknown query error"],
            error
        );
    }
    if (originalRepresentations.count != 1) {
        return GFResolverFailure(
            @"FCPXML asset must contain exactly one original-media representation",
            error
        );
    }
    NSString *source = [
        (NSXMLElement *)originalRepresentations.firstObject attributeForName:@"src"
    ].stringValue;
    NSURL *mediaURL = source.length > 0 ? [NSURL URLWithString:source] : nil;
    if (mediaURL == nil || !mediaURL.isFileURL) {
        return GFResolverFailure(@"original-media src must be a local file URL", error);
    }
    return GFResolution(@"fcpxml", mediaURL, error);
}
