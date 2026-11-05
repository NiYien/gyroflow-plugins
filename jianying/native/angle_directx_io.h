// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once
#include "angle_io.h"
#include <d3d11.h>
#include <wrl/client.h>
#include <cstdint>

struct AngleDirectX {
    HMODULE egl=nullptr;
    void* display=nullptr;
    void* config=nullptr;
    Microsoft::WRL::ComPtr<ID3D11Device> device;
    Microsoft::WRL::ComPtr<ID3D11DeviceContext> context;
    void* (WINAPI *getProc)(const char*)=nullptr;
    void* (WINAPI *createSurface)(void*,unsigned,void*,void*,const int*)=nullptr;
    unsigned (WINAPI *bindImage)(void*,void*,int)=nullptr;
    unsigned (WINAPI *releaseImage)(void*,void*,int)=nullptr;
    unsigned (WINAPI *destroySurface)(void*,void*)=nullptr;
    unsigned (WINAPI *getError)()=nullptr;
    template<class T> T function(const char* name) {
        auto p=GetProcAddress(egl,name);
        if(!p && getProc) p=reinterpret_cast<FARPROC>(getProc(name));
        if(!p) throw std::runtime_error(std::string("Missing EGL symbol: ")+name);
        return reinterpret_cast<T>(p);
    }
    AngleDirectX() {
        egl=GetModuleHandleW(L"libEGL.dll");
        if(!egl) throw std::runtime_error("Host EGL module is not loaded");
        getProc=function<decltype(getProc)>("eglGetProcAddress");
        auto current=function<void*(WINAPI*)()>("eglGetCurrentDisplay");
        auto queryDisplay=function<unsigned(WINAPI*)(void*,int,intptr_t*)>("eglQueryDisplayAttribEXT");
        auto queryDevice=function<unsigned(WINAPI*)(void*,int,intptr_t*)>("eglQueryDeviceAttribEXT");
        auto choose=function<unsigned(WINAPI*)(void*,const int*,void**,int,int*)>("eglChooseConfig");
        getError=function<decltype(getError)>("eglGetError");
        createSurface=function<decltype(createSurface)>("eglCreatePbufferFromClientBuffer");
        bindImage=function<decltype(bindImage)>("eglBindTexImage");
        releaseImage=function<decltype(releaseImage)>("eglReleaseTexImage");
        destroySurface=function<decltype(destroySurface)>("eglDestroySurface");
        display=current();
        intptr_t eglDevice=0,d3dDevice=0;
        if(!display || !queryDisplay(display,0x322C,&eglDevice) || !queryDevice(reinterpret_cast<void*>(eglDevice),0x33A1,&d3dDevice) || !d3dDevice)
            throw std::runtime_error("Cannot query ANGLE D3D11 device");
        device=reinterpret_cast<ID3D11Device*>(d3dDevice);
        device->GetImmediateContext(context.GetAddressOf());
        const int attrs[]={0x3033,1,0x3040,0x40,0x3024,8,0x3023,8,0x3022,8,0x3021,8,0x303A,1,0x3038};
        int count=0;
        if(!choose(display,attrs,&config,1,&count) || !count) throw std::runtime_error("No EGL RGBA pbuffer config");
    }
};

struct AngleD3DTexture {
    AngleDirectX& dx;
    AngleIO& gl;
    Microsoft::WRL::ComPtr<ID3D11Texture2D> texture;
    void* surface=nullptr;
    unsigned glTexture=0;
    bool bound=false;
    int width,height;
    AngleD3DTexture(AngleDirectX& d,AngleIO& g,int w,int h):dx(d),gl(g),width(w),height(h) {
        D3D11_TEXTURE2D_DESC desc{};
        desc.Width=w;desc.Height=h;desc.MipLevels=1;desc.ArraySize=1;
        desc.Format=DXGI_FORMAT_R8G8B8A8_UNORM;desc.SampleDesc.Count=1;
        desc.Usage=D3D11_USAGE_DEFAULT;desc.BindFlags=D3D11_BIND_RENDER_TARGET|D3D11_BIND_SHADER_RESOURCE;
        // Ordinary textures use the core's synchronized shared-copy path.
        HRESULT result=dx.device->CreateTexture2D(&desc,nullptr,texture.GetAddressOf());
        if(FAILED(result)) throw std::runtime_error("D3D11 RGBA texture creation failed: "+std::to_string(result));
        const int attrs[]={0x3080,0x305E,0x3081,0x305F,0x3038};
        surface=dx.createSurface(dx.display,0x33A3,texture.Get(),dx.config,attrs);
        if(!surface) throw std::runtime_error("D3D11 EGL pbuffer creation failed: "+std::to_string(dx.getError()));
        gl.genTextures(1,&glTexture);
    }
    ~AngleD3DTexture() {
        if(bound) dx.releaseImage(dx.display,surface,0x3084);
        if(glTexture) gl.deleteTextures(1,&glTexture);
        if(surface) dx.destroySurface(dx.display,surface);
    }
    void bind() {
        gl.bindTexture(0x0DE1,glTexture);
        if(!dx.bindImage(dx.display,surface,0x3084)) throw std::runtime_error("D3D11 EGL bind failed: "+std::to_string(dx.getError()));
        bound=true;
    }
    void release() {
        if(bound && !dx.releaseImage(dx.display,surface,0x3084)) throw std::runtime_error("D3D11 EGL release failed: "+std::to_string(dx.getError()));
        bound=false;
    }
    void clear() {
        Microsoft::WRL::ComPtr<ID3D11RenderTargetView> view;
        HRESULT result=dx.device->CreateRenderTargetView(texture.Get(),nullptr,view.GetAddressOf());
        if(FAILED(result)) throw std::runtime_error("D3D11 clear view creation failed");
        const float transparent[4]{};
        dx.context->ClearRenderTargetView(view.Get(),transparent);
        dx.context->Flush();
    }
    void copy_from(unsigned source) {
        AngleIO::State state(gl);
        bool scissor=gl.isEnabled(0x0C11)!=0;
        unsigned fbos[2]{};
        bind();
        gl.genFramebuffers(2,fbos);
        gl.bindFramebuffer(0x8CA8,fbos[0]);gl.framebufferTexture2D(0x8CA8,0x8CE0,0x0DE1,source,0);
        gl.bindFramebuffer(0x8CA9,fbos[1]);gl.framebufferTexture2D(0x8CA9,0x8CE0,0x0DE1,glTexture,0);
        bool complete=gl.checkFramebufferStatus(0x8CA8)==0x8CD5 && gl.checkFramebufferStatus(0x8CA9)==0x8CD5;
        if(complete) {
            gl.disable(0x0C11);
            gl.blitFramebuffer(0,0,width,height,0,0,width,height,0x4000,0x2600);
            gl.finish();
        }
        unsigned error=gl.getError();
        if(scissor) gl.enable(0x0C11);
        gl.deleteFramebuffers(2,fbos);
        // EGL ownership must end before D3D11 or Vulkan accesses the resource.
        release();
        if(!complete || error) throw std::runtime_error("ANGLE to D3D11 copy failed: "+std::to_string(error));
    }
    void copy_to(unsigned destination) {
        AngleIO::State state(gl);
        bool scissor=gl.isEnabled(0x0C11)!=0;
        unsigned fbo=0;
        bind();
        gl.genFramebuffers(1,&fbo);gl.bindFramebuffer(0x8CA8,fbo);
        gl.framebufferTexture2D(0x8CA8,0x8CE0,0x0DE1,glTexture,0);
        gl.bindFramebuffer(0x8CA9,destination);
        bool complete=gl.checkFramebufferStatus(0x8CA8)==0x8CD5 && gl.checkFramebufferStatus(0x8CA9)==0x8CD5;
        if(complete) {
            gl.disable(0x0C11);
            gl.blitFramebuffer(0,0,width,height,0,0,width,height,0x4000,0x2600);
            gl.finish();
        }
        unsigned error=gl.getError();
        if(scissor) gl.enable(0x0C11);
        gl.deleteFramebuffers(1,&fbo);
        release();
        if(!complete || error) throw std::runtime_error("D3D11 to ANGLE copy failed: "+std::to_string(error));
    }
    void read(std::vector<unsigned char>& bytes) {
        AngleIO::State state(gl);
        bind();gl.read(glTexture,width,height,bytes);release();
    }
};
