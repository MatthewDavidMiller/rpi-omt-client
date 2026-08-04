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

#pragma once
#include <cstddef>
#include <algorithm>
#include <queue>
#include <mutex>
#include <condition_variable>
#include <functional>
#include <stdexcept>

#if defined(_WIN32)
#include <thread>
#else
#include <pthread.h>
#endif

struct ThreadTask
{
#if defined(_WIN32)
	std::thread thread;
#else
	pthread_t thread{};
	bool threadCreated = false;
#endif
	std::queue<std::function<void()>> queue;
	std::mutex mtx;
	std::condition_variable cv;
	std::condition_variable complete;
	bool running = false;
	bool busy = false;

	void Join()
	{
		std::unique_lock<std::mutex> lock(mtx);
		complete.wait(lock, [this] { return queue.empty() && !busy; });
	}

	void TaskLoop()
	{
		for (;;)
		{
			std::unique_lock<std::mutex> lock(mtx);
			cv.wait(lock, [this] { return !running || !queue.empty(); });
			if (!running) break;
			std::function<void()> func = std::move(queue.front());
			queue.pop();
			busy = true;
			lock.unlock();
			func();
			lock.lock();
			busy = false;
			if (queue.empty()) complete.notify_all();
		}
		complete.notify_all();
	}

#if !defined(_WIN32)
	static void* ThreadEntry(void* context)
	{
		static_cast<ThreadTask*>(context)->TaskLoop();
		return nullptr;
	}
#endif

	void Initialize()
	{
		{
			std::lock_guard<std::mutex> lock(mtx);
			running = true;
			busy = false;
			queue = std::queue<std::function<void()>>();
		}
#if defined(_WIN32)
		thread = std::thread(&ThreadTask::TaskLoop, this);
#else
		pthread_attr_t attributes;
		if (pthread_attr_init(&attributes) != 0)
		{
			throw std::runtime_error("Unable to initialize VMX worker attributes");
		}
		const long minimumStack = PTHREAD_STACK_MIN;
		const std::size_t stackSize = std::max<std::size_t>(
			512U * 1024U, minimumStack > 0 ? static_cast<std::size_t>(minimumStack) : 0U);
		const int stackResult = pthread_attr_setstacksize(&attributes, stackSize);
		const int createResult = stackResult == 0
			? pthread_create(&thread, &attributes, &ThreadTask::ThreadEntry, this)
			: stackResult;
		pthread_attr_destroy(&attributes);
		if (createResult != 0)
		{
			std::lock_guard<std::mutex> lock(mtx);
			running = false;
			throw std::runtime_error("Unable to create bounded-stack VMX worker");
		}
		threadCreated = true;
#endif
	}
	void Push(std::function<void()> task)
	{
		{
			std::lock_guard<std::mutex> lock(mtx);
			queue.push(task);
		}
		cv.notify_all();
	}
	void Destroy()
	{
		{
			std::lock_guard<std::mutex> lock(mtx);
			running = false;
		}
		cv.notify_all();
#if defined(_WIN32)
		if (thread.joinable()) thread.join();
#else
		if (threadCreated)
		{
			pthread_join(thread, nullptr);
			threadCreated = false;
		}
#endif
	}
};

struct ThreadTasks
{
	int numThreads;
	ThreadTask** tasks;
};

[[maybe_unused]] static ThreadTasks* CreateTasks(int numThreads)
{
	ThreadTasks* th = new ThreadTasks();
	th->numThreads = numThreads;
	th->tasks = new ThreadTask*[static_cast<std::size_t>(numThreads)]{};
	try
	{
		for (int i = 0; i < numThreads; i++)
		{
			ThreadTask* task = new ThreadTask();
			th->tasks[i] = task;
			task->Initialize();
		}
	}
	catch (...)
	{
		for (int i = 0; i < numThreads; i++)
		{
			if (th->tasks[i] != nullptr)
			{
				th->tasks[i]->Destroy();
				delete th->tasks[i];
			}
		}
		delete[] th->tasks;
		delete th;
		throw;
	}
	return th;
}

[[maybe_unused]] static void DestroyTasks(ThreadTasks* tasks)
{
	if (tasks)
	{
		for (int i = 0; i < tasks->numThreads; i++)
		{
			tasks->tasks[i]->Destroy();
			ThreadTask* task = tasks->tasks[i];
			delete task;
		}
		delete[] tasks->tasks;
		delete tasks;
	}
}
