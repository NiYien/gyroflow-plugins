// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once
#include <windows.h>
#include <vector>
#include <stdexcept>
#include <string>

// Use the same ANGLE implementation shipped with Jianying's video engine.
struct AngleIO {
    HMODULE module;
    const unsigned char* (WINAPI *getString)(unsigned);
    unsigned (WINAPI *getError)();
    void (WINAPI *getInt)(unsigned,int*);
    void (WINAPI *bindTexture)(unsigned,unsigned);
    void (WINAPI *genFramebuffers)(int,unsigned*);
    void (WINAPI *deleteFramebuffers)(int,const unsigned*);
    void (WINAPI *bindFramebuffer)(unsigned,unsigned);
    void (WINAPI *framebufferTexture2D)(unsigned,unsigned,unsigned,unsigned,int);
    unsigned (WINAPI *checkFramebufferStatus)(unsigned);
    void (WINAPI *readPixels)(int,int,int,int,unsigned,unsigned,void*);
    void (WINAPI *texSubImage2D)(unsigned,int,int,int,int,int,unsigned,unsigned,const void*);
    void (WINAPI *pixelStore)(unsigned,int);
    void (WINAPI *bindBuffer)(unsigned,unsigned);
    void (WINAPI *finish)();
    void (WINAPI *genTextures)(int,unsigned*);
    void (WINAPI *deleteTextures)(int,const unsigned*);
    void (WINAPI *texImage2D)(unsigned,int,int,int,int,int,unsigned,unsigned,const void*);
    void (WINAPI *blitFramebuffer)(int,int,int,int,int,int,int,int,unsigned,unsigned);
    unsigned char (WINAPI *isEnabled)(unsigned);
    void (WINAPI *enable)(unsigned);
    void (WINAPI *disable)(unsigned);
    void (WINAPI *getFramebufferAttachment)(unsigned,unsigned,unsigned,int*);
    template<class T> void load(T& slot,const char* name) {
        slot=reinterpret_cast<T>(GetProcAddress(module,name));
        if(!slot) throw std::runtime_error(std::string("Missing ANGLE symbol: ")+name);
    }
    AngleIO() {
        module=GetModuleHandleW(L"libGLESv2.dll");
        if(!module) throw std::runtime_error("Host ANGLE module is not loaded");
        load(getString,"glGetString");load(getError,"glGetError");load(getInt,"glGetIntegerv");load(bindTexture,"glBindTexture");
        load(genFramebuffers,"glGenFramebuffers");load(deleteFramebuffers,"glDeleteFramebuffers");load(bindFramebuffer,"glBindFramebuffer");
        load(framebufferTexture2D,"glFramebufferTexture2D");load(checkFramebufferStatus,"glCheckFramebufferStatus");
        load(readPixels,"glReadPixels");load(texSubImage2D,"glTexSubImage2D");load(pixelStore,"glPixelStorei");load(bindBuffer,"glBindBuffer");load(finish,"glFinish");
        load(genTextures,"glGenTextures");load(deleteTextures,"glDeleteTextures");load(texImage2D,"glTexImage2D");
        load(blitFramebuffer,"glBlitFramebuffer");load(isEnabled,"glIsEnabled");load(enable,"glEnable");load(disable,"glDisable");
        load(getFramebufferAttachment,"glGetFramebufferAttachmentParameteriv");
    }
    struct State {
        AngleIO& gl;
        bool es3;
        int texture=0,read_fbo=0,draw_fbo=0,pack=4,unpack=4,pack_buffer=0,unpack_buffer=0;
        int stores[6]{};
        const unsigned keys[6]={0x0D02,0x0D03,0x0D04,0x0CF2,0x0CF3,0x0CF4};
        State(AngleIO& g):gl(g) {
            const auto* version=gl.getString(0x1F02);
            if(!version) throw std::runtime_error("No current ANGLE context");
            es3=std::string(reinterpret_cast<const char*>(version)).find("OpenGL ES 3")!=std::string::npos;
            gl.getInt(0x8069,&texture);gl.getInt(0x8CA6,&draw_fbo);read_fbo=draw_fbo;
            gl.getInt(0x0D05,&pack);gl.getInt(0x0CF5,&unpack);
            if(es3) {
                gl.getInt(0x8CAA,&read_fbo);gl.getInt(0x88ED,&pack_buffer);gl.getInt(0x88EF,&unpack_buffer);
                for(int i=0;i<6;++i) gl.getInt(keys[i],&stores[i]);
                gl.bindBuffer(0x88EB,0);gl.bindBuffer(0x88EC,0);
                for(auto key:keys) gl.pixelStore(key,0);
            }
            gl.pixelStore(0x0D05,1);gl.pixelStore(0x0CF5,1);
        }
        ~State() {
            gl.bindTexture(0x0DE1,texture);
            if(es3) {
                gl.bindFramebuffer(0x8CA8,read_fbo);gl.bindFramebuffer(0x8CA9,draw_fbo);
                gl.bindBuffer(0x88EB,pack_buffer);gl.bindBuffer(0x88EC,unpack_buffer);
                for(int i=0;i<6;++i) gl.pixelStore(keys[i],stores[i]);
            } else gl.bindFramebuffer(0x8D40,draw_fbo);
            gl.pixelStore(0x0D05,pack);gl.pixelStore(0x0CF5,unpack);
        }
    };
    void read(unsigned texture,int width,int height,std::vector<unsigned char>& bytes) {
        State state(*this);
        unsigned fbo=0;genFramebuffers(1,&fbo);bindFramebuffer(0x8D40,fbo);
        framebufferTexture2D(0x8D40,0x8CE0,0x0DE1,texture,0);
        if(checkFramebufferStatus(0x8D40)!=0x8CD5) { deleteFramebuffers(1,&fbo);throw std::runtime_error("ANGLE source framebuffer incomplete"); }
        bytes.resize(static_cast<size_t>(width)*height*4);
        readPixels(0,0,width,height,0x1908,0x1401,bytes.data());
        unsigned error=getError();deleteFramebuffers(1,&fbo);
        if(error) throw std::runtime_error("ANGLE read error "+std::to_string(error));
    }
    void write(unsigned texture,int width,int height,const std::vector<unsigned char>& bytes) {
        State state(*this);
        bindTexture(0x0DE1,texture);
        texSubImage2D(0x0DE1,0,0,0,width,height,0x1908,0x1401,bytes.data());
        finish();
        unsigned error=getError();
        if(error) throw std::runtime_error("ANGLE upload error "+std::to_string(error));
    }
    void read_framebuffer(unsigned framebuffer,int width,int height,std::vector<unsigned char>& bytes) {
        State state(*this);
        if(!state.es3) throw std::runtime_error("ANGLE framebuffer read requires GLES3");
        bindFramebuffer(0x8CA8,framebuffer);
        if(checkFramebufferStatus(0x8CA8)!=0x8CD5) throw std::runtime_error("Host read framebuffer incomplete");
        bytes.resize(static_cast<size_t>(width)*height*4);
        readPixels(0,0,width,height,0x1908,0x1401,bytes.data());
        unsigned error=getError();
        if(error) throw std::runtime_error("Host framebuffer read error "+std::to_string(error));
    }
    void write_framebuffer(unsigned framebuffer,int width,int height,const std::vector<unsigned char>& bytes) {
        State state(*this);
        if(!state.es3) throw std::runtime_error("ANGLE framebuffer copy requires GLES3");
        unsigned texture=0,source_fbo=0;
        genTextures(1,&texture);bindTexture(0x0DE1,texture);
        // Upload only to a plugin-owned texture. OFX output is a render target,
        // and imported host textures need not permit TexSubImage2D updates.
        texImage2D(0x0DE1,0,0x8058,width,height,0,0x1908,0x1401,bytes.data());
        unsigned error=getError();
        if(error) {deleteTextures(1,&texture);throw std::runtime_error("Owned texture upload error "+std::to_string(error));}
        genFramebuffers(1,&source_fbo);bindFramebuffer(0x8CA8,source_fbo);
        framebufferTexture2D(0x8CA8,0x8CE0,0x0DE1,texture,0);
        unsigned src_status=checkFramebufferStatus(0x8CA8);
        bindFramebuffer(0x8CA9,framebuffer);
        unsigned dst_status=checkFramebufferStatus(0x8CA9);
        if(src_status!=0x8CD5 || dst_status!=0x8CD5) {
            deleteFramebuffers(1,&source_fbo);deleteTextures(1,&texture);
            throw std::runtime_error("Framebuffer copy incomplete source="+std::to_string(src_status)+" output="+std::to_string(dst_status));
        }
        bool scissor=isEnabled(0x0C11)!=0;
        disable(0x0C11);
        blitFramebuffer(0,0,width,height,0,0,width,height,0x4000,0x2600);
        finish();error=getError();
        if(scissor) enable(0x0C11);
        deleteFramebuffers(1,&source_fbo);deleteTextures(1,&texture);
        if(error) throw std::runtime_error("Host framebuffer copy error "+std::to_string(error));
    }
};
