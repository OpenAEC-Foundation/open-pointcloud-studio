#define _DEFAULT_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <time.h>
#include <wayland-client.h>
#include <linux/input-event-codes.h>
#include "pointer.h"
static struct zwlr_virtual_pointer_manager_v1 *manager;
static void global(void *data, struct wl_registry *registry, uint32_t name, const char *interface, uint32_t version) {
  if (!strcmp(interface, "zwlr_virtual_pointer_manager_v1"))
    manager = wl_registry_bind(registry, name, &zwlr_virtual_pointer_manager_v1_interface, version < 2 ? version : 2);
}
static void removed(void *data, struct wl_registry *registry, uint32_t name) {}
static uint32_t now(void) { struct timespec t; clock_gettime(CLOCK_MONOTONIC, &t); return (uint32_t)(t.tv_sec * 1000 + t.tv_nsec / 1000000); }
int main(int argc, char **argv) {
  if (!getenv("WAYLAND_DISPLAY") || !getenv("XDG_RUNTIME_DIR")) { fprintf(stderr,"An isolated Wayland session is required\n"); return 2; }
  struct wl_display *display = wl_display_connect(NULL);
  if (!display) return 3;
  struct wl_registry *registry = wl_display_get_registry(display);
  const struct wl_registry_listener listener = {global, removed};
  wl_registry_add_listener(registry, &listener, NULL); wl_display_roundtrip(display);
  if (!manager) { fprintf(stderr,"Virtual pointer protocol unavailable\n"); return 4; }
  struct zwlr_virtual_pointer_v1 *pointer = zwlr_virtual_pointer_manager_v1_create_virtual_pointer(manager, NULL);
  for (int i=1; i<argc; i++) {
    if (!strcmp(argv[i],"move") && i+4<argc) {
      uint32_t x=atoi(argv[++i]), y=atoi(argv[++i]), w=atoi(argv[++i]), h=atoi(argv[++i]);
      zwlr_virtual_pointer_v1_motion_absolute(pointer, now(), x, y, w, h);
    } else if (!strcmp(argv[i],"down")) {
      zwlr_virtual_pointer_v1_button(pointer, now(), BTN_LEFT, WL_POINTER_BUTTON_STATE_PRESSED);
    } else if (!strcmp(argv[i],"up")) {
      zwlr_virtual_pointer_v1_button(pointer, now(), BTN_LEFT, WL_POINTER_BUTTON_STATE_RELEASED);
    } else if (!strcmp(argv[i],"wait") && i+1<argc) {
      usleep(atoi(argv[++i])*1000); continue;
    } else { fprintf(stderr,"Unknown input step: %s\n",argv[i]); return 5; }
    zwlr_virtual_pointer_v1_frame(pointer); wl_display_roundtrip(display); usleep(100000);
  }
  zwlr_virtual_pointer_v1_destroy(pointer); wl_display_roundtrip(display); wl_display_disconnect(display); return 0;
}
