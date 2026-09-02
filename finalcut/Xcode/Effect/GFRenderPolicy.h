#import "GyroflowFinalCut.h"

typedef enum GFRenderDisposition {
    GF_RENDER_DISPOSITION_PROCESSED = 0,
    GF_RENDER_DISPOSITION_PASSTHROUGH = 1,
    GF_RENDER_DISPOSITION_HOST_ERROR = 2,
} GFRenderDisposition;

GFRenderDisposition GFRenderDispositionForStatus(GFStatus status);
