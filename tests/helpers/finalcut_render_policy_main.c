#include "GFRenderPolicy.h"

#include <assert.h>

int main(void) {
    assert(GFRenderDispositionForStatus(GF_STATUS_OK) == GF_RENDER_DISPOSITION_PROCESSED);
    assert(GFRenderDispositionForStatus(GF_STATUS_INVALID_PROJECT) == GF_RENDER_DISPOSITION_PASSTHROUGH);
    assert(GFRenderDispositionForStatus(GF_STATUS_MISSING_TIMING) == GF_RENDER_DISPOSITION_PASSTHROUGH);
    assert(GFRenderDispositionForStatus(GF_STATUS_STALE_TIMING) == GF_RENDER_DISPOSITION_PASSTHROUGH);
    assert(GFRenderDispositionForStatus(GF_STATUS_UNSUPPORTED_PIXEL_FORMAT) == GF_RENDER_DISPOSITION_PASSTHROUGH);
    assert(GFRenderDispositionForStatus(GF_STATUS_INVALID_ARGUMENT) == GF_RENDER_DISPOSITION_PASSTHROUGH);
    assert(GFRenderDispositionForStatus(GF_STATUS_RENDER_FAILED) == GF_RENDER_DISPOSITION_HOST_ERROR);
    assert(GFRenderDispositionForStatus(GF_STATUS_PANIC) == GF_RENDER_DISPOSITION_HOST_ERROR);
    return 0;
}
