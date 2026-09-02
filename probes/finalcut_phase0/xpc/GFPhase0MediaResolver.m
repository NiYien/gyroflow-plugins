#import "GFPhase0MediaResolver.h"


static NSString *const GFPhase0ResolverErrorDomain = @"com.niyien.gyroflow.finalcut.phase0-resolver";


static NSDictionary<NSString *, id> * _Nullable GFPhase0ResolverFailure(
    NSString *message,
    NSError **error
) {
    if (error != NULL) {
        *error = [NSError errorWithDomain:GFPhase0ResolverErrorDomain
                                     code:1
                                 userInfo:@{NSLocalizedDescriptionKey : message}];
    }
    return nil;
}


static NSDictionary<NSString *, id> * _Nullable GFPhase0Resolution(
    NSString *source,
    NSURL *mediaURL,
    NSError **error
) {
    if (!mediaURL.isFileURL || mediaURL.path.length == 0) {
        return GFPhase0ResolverFailure(@"original media must be a local file URL", error);
    }

    NSURL *standardizedMediaURL = mediaURL.URLByStandardizingPath;
    NSURL *projectURL = [[standardizedMediaURL URLByDeletingPathExtension]
                         URLByAppendingPathExtension:@"gyroflow"];
    BOOL projectExists = [[NSFileManager defaultManager] fileExistsAtPath:projectURL.path];
    return @{
        @"source" : source,
        @"mediaURL" : standardizedMediaURL.absoluteString,
        @"projectURL" : projectURL.absoluteString,
        @"projectExists" : @(projectExists),
    };
}


NSDictionary<NSString *, id> * _Nullable GFPhase0ResolveFinderURLs(
    NSArray<NSURL *> *urls,
    NSError **error
) {
    if (urls.count != 1) {
        return GFPhase0ResolverFailure(@"drop must contain exactly one Finder file", error);
    }
    NSURL *mediaURL = urls.firstObject;
    if (!mediaURL.isFileURL) {
        return GFPhase0ResolverFailure(@"Finder item must be a local file URL", error);
    }
    return GFPhase0Resolution(@"finder", mediaURL, error);
}


NSDictionary<NSString *, id> * _Nullable GFPhase0ResolveFCPXMLData(
    NSData *data,
    NSError **error
) {
    if (data.length == 0) {
        return GFPhase0ResolverFailure(@"Final Cut pasteboard contains no FCPXML data", error);
    }

    NSError *parseError = nil;
    NSXMLDocument *document = [[NSXMLDocument alloc]
        initWithData:data
             options:NSXMLNodeLoadExternalEntitiesNever
               error:&parseError];
    if (document == nil) {
        return GFPhase0ResolverFailure(
            [NSString stringWithFormat:@"invalid FCPXML: %@", parseError.localizedDescription],
            error
        );
    }

    NSArray<NSXMLNode *> *clipNodes = [document
        nodesForXPath:@"//*[local-name()='asset-clip' and @ref]"
                error:&parseError];
    if (clipNodes == nil) {
        return GFPhase0ResolverFailure(
            [NSString stringWithFormat:@"unable to query FCPXML clips: %@", parseError.localizedDescription],
            error
        );
    }
    if (clipNodes.count != 1) {
        return GFPhase0ResolverFailure(@"FCPXML must contain exactly one clip reference", error);
    }

    NSArray<NSXMLNode *> *assetNodes = [document
        nodesForXPath:@"//*[local-name()='asset']"
                error:&parseError];
    if (assetNodes == nil) {
        return GFPhase0ResolverFailure(
            [NSString stringWithFormat:@"unable to query FCPXML assets: %@", parseError.localizedDescription],
            error
        );
    }
    if (assetNodes.count != 1) {
        return GFPhase0ResolverFailure(@"FCPXML must contain exactly one asset", error);
    }

    NSXMLElement *clip = (NSXMLElement *)clipNodes.firstObject;
    NSXMLElement *asset = (NSXMLElement *)assetNodes.firstObject;
    NSString *clipReference = [clip attributeForName:@"ref"].stringValue;
    NSString *assetIdentifier = [asset attributeForName:@"id"].stringValue;
    if (clipReference.length == 0 || ![clipReference isEqualToString:assetIdentifier]) {
        return GFPhase0ResolverFailure(@"the single clip must reference the single asset", error);
    }

    NSArray<NSXMLNode *> *mediaRepresentations = [asset
        nodesForXPath:@"./*[local-name()='media-rep' and @kind='original-media' and @src]"
                error:&parseError];
    if (mediaRepresentations == nil) {
        return GFPhase0ResolverFailure(
            [NSString stringWithFormat:@"unable to query FCPXML media representations: %@",
                                       parseError.localizedDescription],
            error
        );
    }
    if (mediaRepresentations.count != 1) {
        return GFPhase0ResolverFailure(
            @"FCPXML asset must contain exactly one original-media representation",
            error
        );
    }

    NSXMLElement *originalMedia = (NSXMLElement *)mediaRepresentations.firstObject;
    NSString *source = [originalMedia attributeForName:@"src"].stringValue;
    NSURL *mediaURL = source.length > 0 ? [NSURL URLWithString:source] : nil;
    if (mediaURL == nil || !mediaURL.isFileURL) {
        return GFPhase0ResolverFailure(@"original-media src must be a local file URL", error);
    }
    return GFPhase0Resolution(@"fcpxml", mediaURL, error);
}
