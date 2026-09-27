// fbdraw — 可操作图形界面：帧缓冲绘制 + evdev 键盘交互
#include <unistd.h>
#include <fcntl.h>
#include <sys/mman.h>
#include <sys/ioctl.h>
#include <stdio.h>
#include <string.h>
#include <stdint.h>

struct fb_var { uint32_t xres, yres, xres_v, yres_v, xoff, yoff, bpp, pad[6]; };
#define FBIOGET_VSCREENINFO 0x4600
#define FBIOPUT_VSCREENINFO 0x4601
#define FBIO_FLUSH 0x4610

struct input_event { long sec, usec; uint16_t type, code; int32_t value; };

static uint32_t *fb; static int w, h, fbfd;
// 实测映射: screen R<-V[15:8], G<-V[23:16], B<-V[31:24]
static inline uint32_t rgb(int r,int g,int b){ return ((uint32_t)r<<8)|((uint32_t)g<<16)|((uint32_t)b<<24); }
static void px(int x,int y,uint32_t c){ if(x>=0&&x<w&&y>=0&&y<h) fb[y*w+x]=c; }
static void rect(int x0,int y0,int ww,int hh,uint32_t c){ for(int y=y0;y<y0+hh;y++) for(int x=x0;x<x0+ww;x++) px(x,y,c); }
static void frame(int x0,int y0,int ww,int hh,uint32_t c){ rect(x0,y0,ww,2,c); rect(x0,y0+hh-2,ww,2,c); rect(x0,y0,2,hh,c); rect(x0+ww-2,y0,2,hh,c); }
static void flushfb(void){ ioctl(fbfd, FBIO_FLUSH, 0); }

static int bx=80, by=80, bs=60;
static uint32_t bc; static int paused=0, nkey=0; static char lastkey[8]="-";

static void draw(void){
    rect(0,0,w,h,rgb(28,34,44));                    // 桌面底色
    rect(0,0,w,26,rgb(52,120,198));                 // 顶栏
    rect(0,26,w,3,rgb(20,60,110));                  // 顶栏阴影
    rect(8,6,14,14,rgb(240,240,240));               // “应用”块
    rect(w-120,6,100,14,rgb(30,80,140));            // 状态块
    frame(20,50,220,40,rgb(90,200,120));            // 窗口1
    rect(22,52,216,36,rgb(20,26,34));
    frame(260,50,200,150,rgb(200,160,60));          // 窗口2
    rect(262,52,196,146,rgb(24,30,20));
    for(int i=0;i<8;i++) rect(270+i*22, 120+((i*37)%50), 14, 60+((i*53)%40), rgb(60+i*20,140-i*8,180-i*12)); // 图表
    frame(bx,by,bs,bs,bc);                          // 可移动方块
    rect(bx+4,by+4,bs-8,bs-8,paused?rgb(120,40,40):bc);
    // 状态行（色块编码 nkey）
    rect(20,h-40,300,24,rgb(10,12,16));
    for(int i=0;i<16;i++) rect(24+i*18, h-36, 14, 16, (nkey>>i)&1?rgb(120,220,140):rgb(40,50,60));
    rect(w-260,h-40,12,20,rgb(200,80,60));          // lastkey 色标 r
    rect(w-240,h-40,12,20,rgb(80,200,90));
    rect(w-220,h-40,12,20,rgb(90,120,230));
    if (lastkey[0]=='r') rect(w-260,h-40,12,20,rgb(255,120,100));
    if (lastkey[0]=='g') rect(w-240,h-40,12,20,rgb(120,255,140));
    if (lastkey[0]=='b') rect(w-220,h-40,12,20,rgb(140,160,255));
    flushfb();
}

int main(void){
    setvbuf(stdout, NULL, _IONBF, 0);
    printf("fbdraw: start\n");
    fbfd = open("/dev/fb0", O_RDWR);
    if (fbfd < 0) { printf("fbdraw: no /dev/fb0\n"); return 1; }
    struct fb_var v; memset(&v,0,sizeof v);
    ioctl(fbfd, FBIOGET_VSCREENINFO, &v);
    printf("fbdraw: var %ux%u bpp=%u\n", v.xres, v.yres, v.bpp);
    w = v.xres ? v.xres : 640; h = v.yres ? v.yres : 480;
    if (v.bpp != 32) { printf("fbdraw: bpp=%u unsupported\n", v.bpp); return 1; }
    size_t len = (size_t)w*h*4;
    fb = mmap(0, len, PROT_READ|PROT_WRITE, MAP_SHARED, fbfd, 0);
    printf("fbdraw: mmap=%p\n", fb);
    if (fb == MAP_FAILED) { printf("fbdraw: mmap failed\n"); return 1; }
    bc = rgb(60,200,90);
    draw();
    printf("fbdraw: %dx%d UI up\n", w, h); fflush(stdout);
    int kd = open("/dev/input/event0", O_RDONLY | O_NONBLOCK);
    printf("fbdraw: event0 fd=%d; reading stdin console\n", kd);
    struct input_event ev;
    char c;
    for (;;) {
        // evdev 事件（非阻塞兜底）
        if (kd >= 0) {
            while (read(kd, &ev, sizeof ev) == (ssize_t)sizeof ev) {
                if (ev.type == 1 && ev.value != 0) {
                    if (ev.code==105) c='h'; else if (ev.code==106) c='l';
                    else if (ev.code==103) c='k'; else if (ev.code==108) c='j';
                    else if (ev.code==19) c='r'; else if (ev.code==34) c='g';
                    else if (ev.code==48) c='b'; else if (ev.code==25) c='p';
                    else if (ev.code==46) c='c';
                    else continue;
                    goto have;
                }
            }
        }
        // 串口控制台命令（主输入通道）
        ssize_t n = read(0, &c, 1);
        if (n != 1) continue;
        if (c=='\r'||c=='\n') continue;
    have:
        if (c=='h') bx-=20; else if (c=='l') bx+=20;
        else if (c=='k') by-=20; else if (c=='j') by+=20;
        else if (c=='r') { bc=rgb(230,70,60); lastkey[0]='r'; }
        else if (c=='g') { bc=rgb(70,230,90); lastkey[0]='g'; }
        else if (c=='b') { bc=rgb(80,110,240); lastkey[0]='b'; }
        else if (c=='p') { paused=!paused; lastkey[0]='p'; }
        else if (c=='c') { rect(0,0,w,h,rgb(0,0,0)); flushfb(); printf("fbdraw: clear+quit\n"); return 0; }
        else lastkey[0]=c;
        nkey++;
        draw();
        printf("fbdraw: key %c -> box(%d,%d) keys=%d\n", c, bx, by, nkey);
    }
}
