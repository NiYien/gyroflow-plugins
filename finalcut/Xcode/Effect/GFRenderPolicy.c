#include "GFRenderPolicy.h"

GFRenderDisposition GFRenderDispositionForStatus(GFStatus status) {
    switch (status) {
        case GF_STATUS_OK:
            return GF_RENDER_DISPOSITION_PROCESSED;
        case GF_STATUS_INVALID_PROJECT:
        case GF_STATUS_INVALID_ARGUMENT:
        case GF_STATUS_MISSING_TIMING:
        case GF_STATUS_STALE_TIMING:
        case GF_STATUS_UNSUPPORTED_PIXEL_FORMAT:
            return GF_RENDER_DISPOSITION_PASSTHROUGH;
        default:
            return GF_RENDER_DISPOSITION_HOST_ERROR;
    }
}
