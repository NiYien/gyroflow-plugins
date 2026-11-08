// SPDX-License-Identifier: GPL-3.0-or-later
// The shared OpenFX library performs stabilization; this adapter owns the host bridge.
#define NOMINMAX
#include <windows.h>
#include <ofxImageEffect.h>
#include <ofxParam.h>
#include <ofxProperty.h>
#include <ofxOpenGLRender.h>
#include "angle_io.h"
#include "angle_directx_io.h"
#include "jianying_source_path.h"
#include <memory>
#include <cstring>
#include <cstdlib>
#include <filesystem>
#include <mutex>
#include <string>

namespace fs = std::filesystem;
static OfxPlugin* inner;
static OfxHost* host;
static OfxHost forwarded_host;
static const OfxPropertySuiteV1* prop;
static const OfxParameterSuiteV1* param;
static const OfxImageEffectSuiteV1* image;
static const OfxImageEffectOpenGLRenderSuiteV1* gpu_suite;
static OfxImageEffectSuiteV1 image_proxy;
static OfxPropertySuiteV1 property_proxy;
static thread_local const void* frame_instance;

struct CpuBuffers {std::vector<unsigned char> source,output;};
static thread_local CpuBuffers cached_cpu_buffers;

struct TextureView {
    OfxPropertySetHandle texture_handle=nullptr;
    unsigned texture=0;
    int bounds[4]{};
    std::vector<unsigned char> bytes;
    std::unique_ptr<AngleD3DTexture> directx;
    OfxPropertySetHandle handle() { return reinterpret_cast<OfxPropertySetHandle>(this); }
    int width() const { return bounds[2]-bounds[0]; }
    int height() const { return bounds[3]-bounds[1]; }
};
struct TextureFrame {
    TextureView source,output;
    OfxImageClipHandle source_clip=nullptr,output_clip=nullptr;
    AngleIO* gl=nullptr;
    int output_framebuffer=0;
    std::unique_ptr<AngleDirectX> directx;
};
static thread_local TextureFrame* texture_frame=nullptr;
static TextureView* texture_view(OfxPropertySetHandle p) {
    if(!texture_frame) return nullptr;
    if(p==texture_frame->source.handle()) return &texture_frame->source;
    if(p==texture_frame->output.handle()) return &texture_frame->output;
    return nullptr;
}
static OfxPropertySetHandle real_property_set(OfxPropertySetHandle p) {
    auto* v=texture_view(p);return v ? v->texture_handle : p;
}

static void report_error(const char* reason) {
    const std::string message=std::string("Gyroflow Jianying: ")+reason+"\n";
    OutputDebugStringA(message.c_str());
}

static bool load_texture_view(TextureView& view,OfxImageClipHandle clip,double time) {
    if(view.texture_handle) { gpu_suite->clipFreeTexture(view.texture_handle);view.texture_handle=nullptr; }
    if(gpu_suite->clipLoadTexture(clip,time,kOfxBitDepthByte,nullptr,&view.texture_handle)!=kOfxStatOK) return false;
    int index=0,target=0;
    if(prop->propGetInt(view.texture_handle,kOfxImageEffectPropOpenGLTextureIndex,0,&index)!=0 ||
       prop->propGetInt(view.texture_handle,kOfxImageEffectPropOpenGLTextureTarget,0,&target)!=0 ||
       prop->propGetIntN(view.texture_handle,kOfxImagePropBounds,4,view.bounds)!=0) return false;
    view.texture=static_cast<unsigned>(index);
    return target==0x0DE1 && index>0 && view.width()>0 && view.height()>0;
}

static OfxStatus get_image(OfxImageClipHandle clip, OfxTime time, const OfxRectD* region, OfxPropertySetHandle* result) {
    if(texture_frame) {
        TextureView* view=nullptr;
        if(clip==texture_frame->output_clip) view=&texture_frame->output;
        else if(clip==texture_frame->source_clip) {
            view=&texture_frame->source;
            if(!load_texture_view(*view,clip,time)) return kOfxStatFailed;
            try {
                if(texture_frame->directx) {
                    view->directx=std::make_unique<AngleD3DTexture>(*texture_frame->directx,*texture_frame->gl,view->width(),view->height());
                    view->directx->copy_from(view->texture);
                } else texture_frame->gl->read(view->texture,view->width(),view->height(),view->bytes);
            }
            catch(const std::exception& e) { report_error(e.what());return kOfxStatFailed; }
        }
        if(!view) return kOfxStatErrBadHandle;
        *result=view->handle();
        return kOfxStatOK;
    }
    return image->clipGetImage(clip, time, region, result);
}

static OfxStatus release_image(OfxPropertySetHandle p) {
    return texture_view(p) ? kOfxStatOK : image->clipReleaseImage(p);
}

static const void* fetch_suite(OfxPropertySetHandle h, const char* name, int version) {
    const void* suite = host->fetchSuite(h, name, version);
    if (!std::strcmp(name, kOfxImageEffectSuite) && version == 1 && suite) return &image_proxy;
    if (!std::strcmp(name, kOfxPropertySuite) && version == 1 && suite) return &property_proxy;
    return suite;
}

static OfxStatus get_int(OfxPropertySetHandle p,const char* key,int index,int* value) {
    if(texture_frame && !std::strcmp(key,"NiYienImageEffectPropD3D11Enabled")) { *value=texture_frame->directx ? 1 : 0;return 0; }
    if(texture_frame && !std::strcmp(key,kOfxImageEffectPropOpenGLEnabled)) { *value=0;return 0; }
    if(auto* v=texture_view(p)) if(!std::strcmp(key,kOfxImagePropRowBytes)) { *value=v->width()*4;return 0; }
    return prop->propGetInt(real_property_set(p),key,index,value);
}
static OfxStatus get_int_n(OfxPropertySetHandle p,const char* key,int count,int* value) { return prop->propGetIntN(real_property_set(p),key,count,value); }
static OfxStatus get_double(OfxPropertySetHandle p,const char* key,int index,double* value) { return prop->propGetDouble(real_property_set(p),key,index,value); }
static OfxStatus get_double_n(OfxPropertySetHandle p,const char* key,int count,double* value) { return prop->propGetDoubleN(real_property_set(p),key,count,value); }
static OfxStatus get_string(OfxPropertySetHandle p,const char* key,int index,const char** value) {
    if (!std::strcmp(key,"NiYienJianyingSourcePath") && index == 0 && frame_instance) {
        OfxPropertySetHandle effect_properties = nullptr;
        auto effect = reinterpret_cast<OfxImageEffectHandle>(const_cast<void*>(frame_instance));
        if (image->getPropertySet(effect,&effect_properties) == kOfxStatOK && p == effect_properties) {
            static thread_local std::string source_path;
            source_path = jianying_source_path::copy_source_path(frame_instance);
            if (source_path.empty()) return kOfxStatErrUnknown;
            *value = source_path.c_str();
            return kOfxStatOK;
        }
    }
    if(texture_view(p)) {
        if(!std::strcmp(key,kOfxImageEffectPropPixelDepth)) { *value=kOfxBitDepthByte;return 0; }
        if(!std::strcmp(key,kOfxImageEffectPropComponents)) { *value=kOfxImageComponentRGBA;return 0; }
    }
    return prop->propGetString(real_property_set(p),key,index,value);
}
static OfxStatus get_pointer(OfxPropertySetHandle p,const char* key,int index,void** value) {
    if(auto* v=texture_view(p)) if(v->directx) {
        if(!std::strcmp(key,"NiYienImagePropD3D11Texture")) { *value=v->directx->texture.Get();return 0; }
        if(!std::strcmp(key,"NiYienImagePropD3D11Device")) { *value=texture_frame->directx->device.Get();return 0; }
        if(!std::strcmp(key,"NiYienImagePropD3D11DeviceContext")) { *value=texture_frame->directx->context.Get();return 0; }
    }
    if(auto* v=texture_view(p)) if(!std::strcmp(key,kOfxImagePropData)) { *value=v->bytes.data();return 0; }
    return prop->propGetPointer(real_property_set(p),key,index,value);
}

static void set_host(OfxHost* h) {
    host = h;
    prop = static_cast<const OfxPropertySuiteV1*>(h->fetchSuite(h->host, kOfxPropertySuite, 1));
    param = static_cast<const OfxParameterSuiteV1*>(h->fetchSuite(h->host, kOfxParameterSuite, 1));
    image = static_cast<const OfxImageEffectSuiteV1*>(h->fetchSuite(h->host, kOfxImageEffectSuite, 1));
    gpu_suite=static_cast<const OfxImageEffectOpenGLRenderSuiteV1*>(h->fetchSuite(h->host,kOfxOpenGLRenderSuite,1));
    if (prop) {
        property_proxy=*prop;
        property_proxy.propGetInt=get_int; property_proxy.propGetIntN=get_int_n;
        property_proxy.propGetDouble=get_double; property_proxy.propGetDoubleN=get_double_n;
        property_proxy.propGetString=get_string; property_proxy.propGetPointer=get_pointer;
    }
    if (image) { image_proxy = *image; image_proxy.clipGetImage = get_image; image_proxy.clipReleaseImage = release_image; }
    forwarded_host = *h; forwarded_host.fetchSuite = fetch_suite;
    inner->setHost(&forwarded_host);
}

static OfxStatus texture_render(OfxImageEffectHandle effect,OfxPropertySetHandle in,double time) {
    if(!gpu_suite) return kOfxStatErrMissingHostFeature;
    TextureFrame frame;int status=kOfxStatFailed;
    frame.source.bytes.swap(cached_cpu_buffers.source);frame.output.bytes.swap(cached_cpu_buffers.output);
    try {
        static AngleIO gl;frame.gl=&gl;
        if(!gl.getString(0x1F02)) throw std::runtime_error("No current host ANGLE context");
        if(image->clipGetHandle(effect,"Source",&frame.source_clip,nullptr)!=0 || image->clipGetHandle(effect,"Output",&frame.output_clip,nullptr)!=0) throw std::runtime_error("Missing host clips");
        if(!load_texture_view(frame.output,frame.output_clip,time)) throw std::runtime_error("Cannot load host output texture");
        gl.getInt(0x8CA6,&frame.output_framebuffer);
        gl.getError();
        // Preserve transparent pixels outside any core output region while reusing the allocation.
        const char* directx_setting=std::getenv("NIYIEN_OFX_DIRECTX");
        if(!(directx_setting && !std::strcmp(directx_setting,"0"))) {
            frame.directx=std::make_unique<AngleDirectX>();
            frame.output.directx=std::make_unique<AngleD3DTexture>(*frame.directx,gl,frame.output.width(),frame.output.height());
            frame.output.directx->clear();
        } else frame.output.bytes.assign(static_cast<size_t>(frame.output.width())*frame.output.height()*4,0);
        texture_frame=&frame;
        status=inner->mainEntry(kOfxImageEffectActionRender,effect,in,nullptr);
        texture_frame=nullptr;
        if(status==kOfxStatOK) {
            if(frame.directx) frame.output.directx->copy_to(frame.output_framebuffer);
            else gl.write_framebuffer(frame.output_framebuffer,frame.output.width(),frame.output.height(),frame.output.bytes);
        }
    } catch(const std::exception& e) { report_error(e.what());status=kOfxStatFailed; }
    texture_frame=nullptr;
    if(frame.source.texture_handle) gpu_suite->clipFreeTexture(frame.source.texture_handle);
    if(frame.output.texture_handle) gpu_suite->clipFreeTexture(frame.output.texture_handle);
    frame.source.directx.reset();frame.output.directx.reset();frame.directx.reset();
    frame.source.bytes.swap(cached_cpu_buffers.source);frame.output.bytes.swap(cached_cpu_buffers.output);
    return status;
}

static OfxStatus entry(const char* action, const void* handle, OfxPropertySetHandle in, OfxPropertySetHandle out) {
    if (!inner || !image || !prop || !param) return kOfxStatErrMissingHostFeature;
    auto effect = reinterpret_cast<OfxImageEffectHandle>(const_cast<void*>(handle));
    frame_instance = handle;
    if(!std::strcmp(action,"OfxActionOpenGLContextAttached") || !std::strcmp(action,"OfxActionOpenGLContextDetached") || !std::strcmp(action,"kOfxActionOpenGLContextDetached")) return kOfxStatOK;
    bool render = !std::strcmp(action,kOfxImageEffectActionRender);
    int host_gl=0;
    double time=0;
    if(render) {
        prop->propGetDouble(in,kOfxPropTime,0,&time);
        prop->propGetInt(in,kOfxImageEffectPropOpenGLEnabled,0,&host_gl);
    }
    OfxStatus status = render && host_gl ? texture_render(effect,in,time) : inner->mainEntry(action,handle,in,out);
    if (!std::strcmp(action,kOfxActionDescribe) && status == kOfxStatOK) {
        OfxPropertySetHandle props = nullptr; image->getPropertySet(effect,&props);
        for (auto key : {kOfxPropLabel,kOfxPropShortLabel,kOfxPropLongLabel}) prop->propSetString(props,key,0,"Gyroflow");
        prop->propSetString(props,kOfxImageEffectPluginPropGrouping,0,"NiYien");
        const char* depth[] = {kOfxBitDepthByte}; prop->propSetStringN(props,kOfxImageEffectPropSupportedPixelDepths,1,depth);
        for (auto key : {"OfxImageEffectPropOpenGLRenderSupported","OfxImageEffectPropOpenCLRenderSupported","OfxImageEffectPropCudaRenderSupported","OfxImageEffectPropMetalRenderSupported","OfxImageEffectPropAngleRenderSupported"}) prop->propSetString(props,key,0,"false");
        // Jianying prefers desktop OpenGL over ANGLE when both are advertised.
        prop->propSetString(props,"OfxImageEffectPropAngleRenderSupported",0,"true");
    }
    if (!std::strcmp(action,kOfxImageEffectActionDescribeInContext) && status == kOfxStatOK) {
        // Keep the serialized parameter identity for existing drafts; rendering always stabilizes.
        OfxParamSetHandle params; OfxPropertySetHandle p;
        image->getParamSet(effect,&params);
        if (param->paramDefine(params,kOfxParamTypeChoice,"DiagnosticMode",&p) == kOfxStatOK) {
            prop->propSetString(p,kOfxPropLabel,0,"Diagnostic mode");
            prop->propSetString(p,kOfxParamPropHint,0,"Internal rendering diagnostics.");
            prop->propSetInt(p,kOfxParamPropDefault,0,0);
            prop->propSetInt(p,kOfxParamPropAnimates,0,0);
            prop->propSetInt(p,kOfxParamPropSecret,0,1);
            const char* choices[] = {"0 - Stabilize","1 - Source copy","2 - Color pattern","3 - Source opaque","4 - Batch sweep (2.5s each)"}; prop->propSetStringN(p,kOfxParamPropChoiceOption,5,choices);
            OfxParamHandle page; OfxPropertySetHandle page_props;
            if (param->paramGetHandle(params,"Main",&page,&page_props) == kOfxStatOK) { int count=0; prop->propGetDimension(page_props,kOfxParamPropPageChild,&count); prop->propSetString(page_props,kOfxParamPropPageChild,count,"DiagnosticMode"); }
        }
    }
    return status;
}

static OfxPlugin plugin = {kOfxImageEffectPluginApi,1,"xyz.NiYien.gyroflow",1,11,set_host,entry};
extern "C" __declspec(dllexport) int OfxGetNumberOfPlugins() { return 1; }
extern "C" __declspec(dllexport) OfxPlugin* OfxGetPlugin(int index) {
    if (index != 0) return nullptr;
    static std::once_flag once;
    std::call_once(once, [] {
        HMODULE own = nullptr; wchar_t path[32768]{};
        GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,reinterpret_cast<LPCWSTR>(&OfxGetPlugin),&own);
        GetModuleFileNameW(own,path,32768);
        auto binary_dir = fs::path(path).parent_path();
        HMODULE dll = LoadLibraryW((binary_dir / L"gyroflow_ofx.dll").c_str());
        if (!dll) { report_error("Cannot load gyroflow_ofx.dll");return; }
        auto get = reinterpret_cast<OfxPlugin*(*)(int)>(GetProcAddress(dll,"OfxGetPlugin"));
        if (get) inner = get(0);
    });
    return inner ? &plugin : nullptr;
}
