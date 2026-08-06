/*
* MIT License
*
* Copyright (c) 2025 Open Media Transport Contributors
*
* Permission is hereby granted, free of charge, to any person obtaining a copy
* of this software and associated documentation files (the "Software"), to deal
* in the Software without restriction, including without limitation the rights
* to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
* copies of the Software, and to permit persons to whom the Software is
* furnished to do so, subject to the following conditions:
*
* The above copyright notice and this permission notice shall be included in all
* copies or substantial portions of the Software.
*
* THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
* IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
* FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
* AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
* LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
* OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
* SOFTWARE.
*
*/

#include "thread_tasks.h"

#include <stdint.h>
#include <stdlib.h>

#if defined(_WIN32)
#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <windows.h>
#else
#include <pthread.h>
#endif

#define VMX_THREAD_STACK_SIZE (512U * 1024U)

struct ThreadTask {
#if defined(_WIN32)
	HANDLE thread;
	CRITICAL_SECTION mutex;
	CONDITION_VARIABLE ready;
	CONDITION_VARIABLE complete;
#else
	pthread_t thread;
	pthread_mutex_t mutex;
	pthread_cond_t ready;
	pthread_cond_t complete;
#endif
	VMX_TASK_FUNCTION function;
	void *context;
	int start_index;
	int count;
	int running;
	int pending;
	int busy;
	int thread_created;
};

#if defined(_WIN32)
static DWORD WINAPI VMX_ThreadEntry(LPVOID context)
#else
static void *VMX_ThreadEntry(void *context)
#endif
{
	ThreadTask *task = (ThreadTask *)context;
	for (;;) {
#if defined(_WIN32)
		EnterCriticalSection(&task->mutex);
		while (task->running && !task->pending) {
			(void)SleepConditionVariableCS(&task->ready, &task->mutex, INFINITE);
		}
#else
		(void)pthread_mutex_lock(&task->mutex);
		while (task->running && !task->pending) {
			(void)pthread_cond_wait(&task->ready, &task->mutex);
		}
#endif
		if (!task->running) {
#if defined(_WIN32)
			WakeAllConditionVariable(&task->complete);
			LeaveCriticalSection(&task->mutex);
			return 0;
#else
			(void)pthread_cond_broadcast(&task->complete);
			(void)pthread_mutex_unlock(&task->mutex);
			return NULL;
#endif
		}
		VMX_TASK_FUNCTION function = task->function;
		void *function_context = task->context;
		int start_index = task->start_index;
		int count = task->count;
		task->pending = 0;
		task->busy = 1;
#if defined(_WIN32)
		LeaveCriticalSection(&task->mutex);
#else
		(void)pthread_mutex_unlock(&task->mutex);
#endif
		function(function_context, start_index, count);
#if defined(_WIN32)
		EnterCriticalSection(&task->mutex);
		task->busy = 0;
		WakeAllConditionVariable(&task->complete);
		LeaveCriticalSection(&task->mutex);
#else
		(void)pthread_mutex_lock(&task->mutex);
		task->busy = 0;
		(void)pthread_cond_broadcast(&task->complete);
		(void)pthread_mutex_unlock(&task->mutex);
#endif
	}
}

static int VMX_ThreadTaskInitialize(ThreadTask *task)
{
#if defined(_WIN32)
	InitializeCriticalSection(&task->mutex);
	InitializeConditionVariable(&task->ready);
	InitializeConditionVariable(&task->complete);
	task->running = 1;
	task->thread =
		CreateThread(NULL, VMX_THREAD_STACK_SIZE, VMX_ThreadEntry, task, 0, NULL);
	if (task->thread == NULL) {
		task->running = 0;
		DeleteCriticalSection(&task->mutex);
		return 0;
	}
#else
	pthread_attr_t attributes;
	size_t stack_size = VMX_THREAD_STACK_SIZE;
	if (pthread_mutex_init(&task->mutex, NULL) != 0) return 0;
	if (pthread_cond_init(&task->ready, NULL) != 0) {
		(void)pthread_mutex_destroy(&task->mutex);
		return 0;
	}
	if (pthread_cond_init(&task->complete, NULL) != 0) {
		(void)pthread_cond_destroy(&task->ready);
		(void)pthread_mutex_destroy(&task->mutex);
		return 0;
	}
	if (pthread_attr_init(&attributes) != 0) goto fail_posix;
#if defined(PTHREAD_STACK_MIN)
	if ((size_t)PTHREAD_STACK_MIN > stack_size) stack_size = (size_t)PTHREAD_STACK_MIN;
#endif
	if (pthread_attr_setstacksize(&attributes, stack_size) != 0) {
		(void)pthread_attr_destroy(&attributes);
		goto fail_posix;
	}
	task->running = 1;
	if (pthread_create(&task->thread, &attributes, VMX_ThreadEntry, task) != 0) {
		task->running = 0;
		(void)pthread_attr_destroy(&attributes);
		goto fail_posix;
	}
	(void)pthread_attr_destroy(&attributes);
#endif
	task->thread_created = 1;
	return 1;
#if !defined(_WIN32)
fail_posix:
	(void)pthread_cond_destroy(&task->complete);
	(void)pthread_cond_destroy(&task->ready);
	(void)pthread_mutex_destroy(&task->mutex);
	return 0;
#endif
}

int ThreadTaskPush(ThreadTask *task, VMX_TASK_FUNCTION function,
	void *context, int start_index, int count)
{
	int accepted = 0;
#if defined(_WIN32)
	EnterCriticalSection(&task->mutex);
#else
	(void)pthread_mutex_lock(&task->mutex);
#endif
	if (task->running && !task->pending && !task->busy) {
		task->function = function;
		task->context = context;
		task->start_index = start_index;
		task->count = count;
		task->pending = 1;
		accepted = 1;
#if defined(_WIN32)
		WakeConditionVariable(&task->ready);
#else
		(void)pthread_cond_signal(&task->ready);
#endif
	}
#if defined(_WIN32)
	LeaveCriticalSection(&task->mutex);
#else
	(void)pthread_mutex_unlock(&task->mutex);
#endif
	return accepted;
}

void ThreadTaskJoin(ThreadTask *task)
{
#if defined(_WIN32)
	EnterCriticalSection(&task->mutex);
	while (task->pending || task->busy) {
		(void)SleepConditionVariableCS(&task->complete, &task->mutex, INFINITE);
	}
	LeaveCriticalSection(&task->mutex);
#else
	(void)pthread_mutex_lock(&task->mutex);
	while (task->pending || task->busy) {
		(void)pthread_cond_wait(&task->complete, &task->mutex);
	}
	(void)pthread_mutex_unlock(&task->mutex);
#endif
}

static void VMX_ThreadTaskDestroy(ThreadTask *task)
{
	if (task == NULL || !task->thread_created) return;
#if defined(_WIN32)
	EnterCriticalSection(&task->mutex);
	task->running = 0;
	WakeAllConditionVariable(&task->ready);
	LeaveCriticalSection(&task->mutex);
	(void)WaitForSingleObject(task->thread, INFINITE);
	(void)CloseHandle(task->thread);
	DeleteCriticalSection(&task->mutex);
#else
	(void)pthread_mutex_lock(&task->mutex);
	task->running = 0;
	(void)pthread_cond_broadcast(&task->ready);
	(void)pthread_mutex_unlock(&task->mutex);
	(void)pthread_join(task->thread, NULL);
	(void)pthread_cond_destroy(&task->complete);
	(void)pthread_cond_destroy(&task->ready);
	(void)pthread_mutex_destroy(&task->mutex);
#endif
	task->thread_created = 0;
}

void DestroyTasks(ThreadTasks *tasks)
{
	if (tasks == NULL) return;
	for (int index = 0; index < tasks->numThreads; ++index) {
		if (tasks->tasks[index] != NULL) {
			VMX_ThreadTaskDestroy(tasks->tasks[index]);
			free(tasks->tasks[index]);
		}
	}
	free(tasks->tasks);
	free(tasks);
}

ThreadTasks *CreateTasks(int num_threads)
{
	ThreadTasks *tasks;
	if (num_threads <= 0 || (size_t)num_threads > SIZE_MAX / sizeof(ThreadTask *)) {
		return NULL;
	}
	tasks = (ThreadTasks *)calloc(1, sizeof(*tasks));
	if (tasks == NULL) return NULL;
	tasks->tasks = (ThreadTask **)calloc((size_t)num_threads, sizeof(*tasks->tasks));
	if (tasks->tasks == NULL) {
		free(tasks);
		return NULL;
	}
	tasks->numThreads = num_threads;
	for (int index = 0; index < num_threads; ++index) {
		tasks->tasks[index] = (ThreadTask *)calloc(1, sizeof(*tasks->tasks[index]));
		if (tasks->tasks[index] == NULL ||
			!VMX_ThreadTaskInitialize(tasks->tasks[index])) {
			DestroyTasks(tasks);
			return NULL;
		}
	}
	return tasks;
}
