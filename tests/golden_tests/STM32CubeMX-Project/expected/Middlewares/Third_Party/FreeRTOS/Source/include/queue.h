#ifndef QUEUE_H
#define QUEUE_H

#ifndef INC_FREERTOS_H
	#error "include FreeRTOS.h" must appear in source files before "include queue.h"
#endif

#ifdef __cplusplus
extern "C" {
#endif

#include "task.h"  // IWYU: export

#ifdef __cplusplus
}
#endif

#endif /* QUEUE_H */
